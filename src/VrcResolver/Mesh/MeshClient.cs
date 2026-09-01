using System.Collections.Concurrent;
using System.Net.WebSockets;
using System.Reflection;
using System.Text.Json;
using MessagePack;
using MessagePack.Resolvers;
using VrcResolver.Shared;

namespace VrcResolver;

internal readonly record struct MeshResolveResult(byte[] Frame, string Action, string? Reason, int? RetryAfterMs = null);

internal sealed partial class MeshClient : IAsyncDisposable
{
    private static readonly TimeSpan ApexAttemptTimeout = TimeSpan.FromSeconds(10);
    private static readonly TimeSpan HeartbeatInterval = TimeSpan.FromSeconds(45);
    private static readonly TimeSpan PongDeadline = TimeSpan.FromSeconds(10);
    private static readonly TimeSpan ApexReResolveAfter = TimeSpan.FromMinutes(5);
    private static readonly TimeSpan WelcomeTimeout = TimeSpan.FromSeconds(1);
    private static readonly int[] ReconnectCapsSec = { 1, 2, 4, 8, 16, 30 };

    private static readonly byte[] PingFrame = "{\"action\":\"ping\"}"u8.ToArray();
    private static readonly byte[] PongFrame = "{\"action\":\"pong\"}"u8.ToArray();

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

    private readonly string _clientId = ClientIdentity.LoadOrCreate();

    private readonly object _recentCidsLock = new();
    private readonly Dictionary<string, (string Cid, DateTime At)> _recentCids = new();
    private const int MaxRecentCids = 256;
    private static readonly TimeSpan RecentCidsTtl = TimeSpan.FromHours(1);

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

    private bool _isV3Connection;
    private string _currentNodeHost = "";
    private readonly WelcomeCache _welcomeCache = new();

    private string _negotiatedFormat = WireConstants.FormatJson;
    private bool _isMsgpackFormat;

    private TaskCompletionSource<WelcomeFrame?>? _welcomeTcs;
    private int _serverProtocolVersion;
    private string? _serverNode;
    private string[]? _serverFeatures;
    private bool? _warpActive;
    private string? _serverVersion;
    private string? _ytDlpVersion;

    private const int MaxRateLimitCooldownSeconds = 60;
    private long _resolveRateLimitedUntilTicks;

    private static bool HasFeature(string[]? features, string feature)
        => features != null && Array.IndexOf(features, feature) >= 0;

    private bool ServerHasFeature(string feature) => HasFeature(_serverFeatures, feature);

    public bool IsConnected => _ws?.State == WebSocketState.Open;
    public int ServerProtocolVersion => _serverProtocolVersion;
    public string? ServerNode => _serverNode;
    public string CurrentNodeHost => _currentNodeHost;
    public bool? WarpActive => _warpActive;

    internal static bool ShouldSendClientHello(string? negotiatedSubprotocol)
        => string.Equals(negotiatedSubprotocol, WireConstants.SubprotocolV3, StringComparison.Ordinal);

    private async Task SendClientHelloAsync(string nodeHost, CancellationToken ct)
    {
        var ws = _ws;
        if (ws is not { State: System.Net.WebSockets.WebSocketState.Open }) return;
        string? cachedHash = _welcomeCache.Get(nodeHost)?.WelcomeHash;
        var hello = new ClientHelloFrame
        {
            WelcomeHash = cachedHash,
            ClientId = _clientId,
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
        catch { }
        if (_runner != null)
        {
            try { await _runner.ConfigureAwait(false); } catch { }
        }
    }

    private static string CidSuffix(string? correlationId) =>
        string.IsNullOrEmpty(correlationId) ? "" : " cid=" + LogUtil.SanitizeForConsole(correlationId, 64);

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
        try { _ws?.Dispose(); } catch { }
        _ws = null;
        _httpClient.Dispose();
        _runCts?.Dispose();
    }
}
