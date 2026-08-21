using System.Collections.Concurrent;
using System.Net.WebSockets;
using System.Reflection;
using System.Text.Json;
using MessagePack;
using MessagePack.Resolvers;
using VrcResolver.Shared;

namespace VrcResolver;

// Carries the verified raw JSON bytes from a `resolved` / `fallback_native`
// WS frame plus the action + server-supplied reason already extracted by
// the dispatch handler. Letting the caller (LocalIpcServer) write Frame
// straight through to the pipe avoids a JsonDocument re-encode on the hot
// path; passing Action/Reason through avoids a second TryGetProperty parse
// for the user-facing console summary.
internal readonly record struct MeshResolveResult(byte[] Frame, string Action, string? Reason);

// Persistent reconnecting WebSocket client to the server's mesh endpoint.
//
// Primary path: connect directly to ServerEndpoints.ProxyHost and let edge routing pick
// an origin. If that host is unavailable before a connection is established,
// fall back to the legacy apex-302 discovery shape so older deployment layouts
// that return node aliases still work. The selected hostname is cached in
// memory and re-resolved if reconnect attempts keep failing for more than
// 5 minutes straight.
//
// v2 protocol: on each new WS connection the server emits a one-shot "welcome"
// frame ~50ms after accept carrying its protocol_version, node, features,
// warp_active, and version strings. Clients wait up to 1s for it before
// sending the first resolve; absent welcome → assume v1 server. Once welcome
// confirms v2, ResolveAsync stamps protocol_version=2 on outgoing requests
// (unless the patched yt-dlp already set it) so the server emits v2 response
// fields (container, video_codec, audio_codec, protocol, audio_channels,
// bytes_estimate, expires_at).
//
// Public surface: ResolveAsync takes the WHOLE ResolveRequest DTO so unknown
// fields (and v2 fields the patched yt-dlp populated) round-trip losslessly
// to the server.
internal sealed partial class MeshClient : IAsyncDisposable
{
    private static readonly TimeSpan ApexAttemptTimeout = TimeSpan.FromSeconds(10);
    private static readonly TimeSpan HeartbeatInterval = TimeSpan.FromSeconds(45);
    private static readonly TimeSpan PongDeadline = TimeSpan.FromSeconds(10);
    private static readonly TimeSpan ApexReResolveAfter = TimeSpan.FromMinutes(5);
    private static readonly TimeSpan WelcomeTimeout = TimeSpan.FromSeconds(1);
    private static readonly int[] ReconnectCapsSec = { 1, 2, 4, 8, 16, 30 };

    // Pre-baked control frames. Both are pure-static -- `{"action":"ping"}`
    // / `{"action":"pong"}` -- so the byte[] can be cached at class-load
    // and reused for every send. Pre-fix each heartbeat / pong-reply
    // allocated a fresh anonymous-object DTO + ArrayPool buffer.
    //
    // AOT migration: anonymous types can't be source-genned (no class
    // declaration to attach [JsonSerializable] to), so the original
    // `JsonSerializer.SerializeToUtf8Bytes(new { action = "ping" })`
    // would fall back to reflection at class-load -- which under AOT
    // throws PlatformNotSupportedException and crashes the watchdog
    // before Main runs. Replaced with UTF-8 string literals that
    // produce identical wire bytes (verified byte-exact by the existing
    // wire-protocol tests).
    private static readonly byte[] PingFrame = "{\"action\":\"ping\"}"u8.ToArray();
    private static readonly byte[] PongFrame = "{\"action\":\"pong\"}"u8.ToArray();

    // AOT-clean MessagePack options. CompositeResolver chains the
    // source-gen resolver (knows our [MessagePackObject] types) and
    // BuiltinResolver (knows primitives + System.String etc.). NOT
    // StandardResolver: its static reference to DynamicObjectResolver +
    // DynamicGenericResolver pulls Reflection.Emit code paths that
    // throw PlatformNotSupportedException under AOT.
    //
    // Probe-validated end-to-end: see project_v3_1_msgpack_client.md.
    // Cached static so each Deserialize call in DispatchBinaryFrameAsync
    // doesn't reconstruct the resolver chain.
    private static readonly MessagePackSerializerOptions s_msgpackOpts =
        MessagePackSerializerOptions.Standard.WithResolver(
            CompositeResolver.Create(
                MeshMsgpackResolver.Instance,
                BuiltinResolver.Instance));

    private readonly string _userAgent;
    private readonly HttpClient _httpClient;
    private readonly ConcurrentDictionary<string, TaskCompletionSource<MeshResolveResult>> _pending = new();
    private readonly Random _rng = new();
    private readonly SemaphoreSlim _sendGate = new(1, 1);

    // Stable per-install identity included on every playback_feedback frame
    // and v3 client_hello as `client_id`. Server logs it as
    // `reported_client_id` (the server tags its own connection-side id
    // separately) so an operator can join watchdog logs with
    // server-side failures.jsonl entries without having to match on socket
    // address — and a returning watchdog presents the same identity across
    // launches. Persisted at %LOCALAPPDATA%Low\vrcresolver\client_id.txt;
    // see ClientIdentity for the load/create flow.
    private readonly string _clientId = ClientIdentity.LoadOrCreate();

    // Recent (resolved-url → correlation_id) mapping populated when the server
    // returns a `resolved` frame. VrcLogMonitor consults this when emitting
    // playback_feedback so the server's dispatcher can hit its correlation
    // cache (TTL 1h) and attribute the failure to the exact (domain, config)
    // pair instead of falling back to URL-host extraction for a first-party
    // playback proxy URL.
    private readonly object _recentCidsLock = new();
    private readonly Dictionary<string, (string Cid, DateTime At)> _recentCids = new();
    private const int MaxRecentCids = 256;
    private static readonly TimeSpan RecentCidsTtl = TimeSpan.FromHours(1);

    // In-flight (request_id → cid) so the resolved-frame handler can lift the
    // cid out of the originating request and stash it under the resolved URL.
    // Patched yt-dlp populates correlation_id when it knows; otherwise we use
    // the request's `id` so the server can still cache-key on something stable.
    private readonly ConcurrentDictionary<string, string> _inflightCids = new();

    private ClientWebSocket? _ws;
    private string? _cachedNodeHost;
    private CancellationTokenSource? _runCts;
    private Task? _runner;
    private DateTime _firstReconnectFailureUtc = DateTime.MinValue;
    private DateTime _lastPongUtc = DateTime.MinValue;
    private int _reconnectAttempt;
    private bool _wasConnected;
    private bool _useApexDiscoveryFallback;

    // v3 handshake state (per-connection). _isV3Connection is set on
    // ConnectAsync return based on the server's echoed subprotocol; if
    // it doesn't come back as SubprotocolV3 we fall back to the v2 path
    // (skip client_hello, wait for plain welcome). _currentNodeHost is
    // captured per-connection so the welcome-cache lookup keys on the
    // exact public host we connected to. The current release uses
    // ServerEndpoints.ProxyHost on the happy path; legacy apex fallback may still
    // return node aliases.
    private bool _isV3Connection;
    private string _currentNodeHost = "";
    private readonly WelcomeCache _welcomeCache = new();

    // v3.1: post-welcome wire format the server selected for THIS
    // connection. Set on welcome / welcome_cached receipt from the
    // negotiated_format field; null/missing field defaults to
    // FormatJson (v3.0 behaviour). Drives the receive-loop branch on
    // WebSocketMessageType.Binary — only honoured when this is "msgpack".
    private string _negotiatedFormat = WireConstants.FormatJson;
    private bool _isMsgpackFormat;

    // Per-connection welcome state. _welcomeTcs is reset on every successful
    // ConnectAsync; the 1s fallback completes it with null if the server is
    // pre-v2 (silent) and the dispatch handler completes it with the parsed
    // frame on welcome arrival. _serverProtocolVersion is 0 = pre-welcome,
    // 1 = no welcome arrived (assume v1 server), 2 = v2 confirmed.
    private TaskCompletionSource<WelcomeFrame?>? _welcomeTcs;
    private int _serverProtocolVersion;
    private string? _serverNode;
    private string[]? _serverFeatures;
    private bool? _warpActive;
    private string? _serverVersion;
    private string? _ytDlpVersion;

    // Set by a rate_limited(meshAction=resolve) frame; ResolveAsync short-circuits
    // to a synthesized rate_limited fallback until it passes so the cooldown never
    // costs wire traffic. Clamp keeps a bad server value from parking resolving.
    private const int MaxRateLimitCooldownSeconds = 60;
    private long _resolveRateLimitedUntilTicks;

    private static bool HasFeature(string[]? features, string feature)
        => features != null && Array.IndexOf(features, feature) >= 0;

    private bool ServerHasFeature(string feature) => HasFeature(_serverFeatures, feature);

    public bool IsConnected => _ws?.State == WebSocketState.Open;
    public int ServerProtocolVersion => _serverProtocolVersion;
    public string? ServerNode => _serverNode;
    // Hostname of the WS endpoint we're currently connected to (usually
    // ServerEndpoints.ProxyHost). Used by ResolveCache as part of the cache
    // key so different mesh nodes never cross-serve cached URLs.
    // Distinct from ServerNode -- ServerNode is the server-supplied
    // logical node label from the welcome frame; CurrentNodeHost is
    // the DNS hostname we resolved to.
    public string CurrentNodeHost => _currentNodeHost;
    public bool? WarpActive => _warpActive;

    // Pure predicate so the subprotocol-mismatch fallback can be unit-
    // tested without standing up a real ClientWebSocket. True iff the
    // server echoed the v3 subprotocol literal back on the upgrade —
    // null/empty/anything-else means the server (or an intermediate
    // proxy stripping unrecognized headers, e.g. some Cloudflare
    // configs) didn't accept v3, and the client must fall back to the
    // v2 path: skip client_hello, wait for plain welcome.
    internal static bool ShouldSendClientHello(string? negotiatedSubprotocol)
        => string.Equals(negotiatedSubprotocol, WireConstants.SubprotocolV3, StringComparison.Ordinal);

    // Send the v3 first frame. Looks up any cached welcome hash for the
    // current node so the server can reply with a small welcome_cached
    // frame on a match instead of resending the full welcome bytes.
    private async Task SendClientHelloAsync(string nodeHost, CancellationToken ct)
    {
        var ws = _ws;
        if (ws is not { State: System.Net.WebSockets.WebSocketState.Open }) return;
        string? cachedHash = _welcomeCache.Get(nodeHost)?.WelcomeHash;
        var hello = new ClientHelloFrame
        {
            WelcomeHash = cachedHash,
            ClientId = _clientId,
            // v3.1: prefer msgpack on the post-welcome hot path,
            // fall back to json. Server picks the first format from
            // this list that it supports — v3.0 servers (or v3.1
            // servers that fail to advertise msgpack_format) just
            // pick "json" and the connection runs as v3.0. Binary
            // frame dispatch is gated on _isMsgpackFormat, set on
            // welcome / welcome_cached receipt from the
            // negotiated_format field.
            AcceptFormats = WireConstants.AcceptFormatsPreference,
        };
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(hello, MeshJsonContext.Default.ClientHelloFrame);
        await SendTextFrameAsync(bytes, ct).ConfigureAwait(false);
        Logger.WriteFileOnly("[mesh][v3] client_hello sent node=" + nodeHost
            + " hash=" + (cachedHash ?? "null"));
    }

    public MeshClient()
    {
        var ver = Assembly.GetExecutingAssembly().GetName().Version?.ToString() ?? "0.0.0.0";
        _userAgent = "VRCResolver-Watchdog/" + ver;
        var handler = new HttpClientHandler { AllowAutoRedirect = false };
        _httpClient = new HttpClient(handler) { Timeout = ApexAttemptTimeout };
    }

    public Task StartAsync()
    {
        _runCts = new CancellationTokenSource();
        _runner = Task.Run(() => RunLoopAsync(_runCts.Token));
        return Task.CompletedTask;
    }

    public async Task StopAsync()
    {
        _runCts?.Cancel();
        FailAllPending(WireConstants.FallbackServerUnreachable);
        try
        {
            if (_ws?.State == WebSocketState.Open)
            {
                using var cts = new CancellationTokenSource(2000);
                await _ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "shutting down", cts.Token).ConfigureAwait(false);
            }
        }
        catch { /* best-effort */ }
        if (_runner != null)
        {
            try { await _runner.ConfigureAwait(false); } catch { /* ignore */ }
        }
    }

    // Format helper for log lines, only when correlation_id is populated.
    private static string CidSuffix(string? correlationId) =>
        string.IsNullOrEmpty(correlationId) ? "" : " cid=" + LogUtil.SanitizeForConsole(correlationId, 64);

    // False = the socket was not open at either check and nothing was sent.
    // ResolveAsync must treat that as a failed send -- a silent no-op here
    // used to leave its pending TaskCompletionSource waiting out the whole
    // IPC budget with nothing ever coming back.
    private async Task<bool> SendTextFrameAsync(byte[] payload, CancellationToken ct)
    {
        var ws = _ws;
        if (ws is not { State: WebSocketState.Open }) return false;

        await _sendGate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            if (ws.State != WebSocketState.Open) return false;
            await ws.SendAsync(payload, WebSocketMessageType.Text, true, ct).ConfigureAwait(false);
            return true;
        }
        finally
        {
            _sendGate.Release();
        }
    }

    public async ValueTask DisposeAsync()
    {
        await StopAsync().ConfigureAwait(false);
        // StopAsync's CloseAsync attempt may have left _ws in a closed-but-
        // not-disposed state if the timeout fired or the close threw. The run
        // loop's finally also disposes _ws on normal exit, but DisposeAsync
        // is the catch-all for "make sure no socket is still pinning a handle."
        try { _ws?.Dispose(); } catch { /* best-effort */ }
        _ws = null;
        _httpClient.Dispose();
        _runCts?.Dispose();
    }
}
