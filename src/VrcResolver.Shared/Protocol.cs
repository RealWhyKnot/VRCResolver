// Wire types and constants shared between the watchdog client (vrcresolver.exe)
// and the patched yt-dlp.exe over the local named pipe, and between the watchdog
// and the mesh server over the WebSocket. Both sides serialize the same
// JSON shapes; only the transport differs.
//
// Schema versioning:
//   v1 — original shape (action/id/url/player/maxHeight on request; action/id/
//        url/engine/config/reason/message on response).
//   v2 — adds optional snake_case request fields (protocol_version, correlation_id,
//        accept_protocols, accept_codecs, max_audio_channels, vrchat_format_arg)
//        and optional response fields (container, video_codec, audio_codec,
//        protocol, audio_channels, bytes_estimate, expires_at). Plus a server-
//        emitted "welcome" frame on WS connect carrying server capabilities.
//
// Backward-compat: every DTO uses [JsonExtensionData] so unknown fields round-trip
// across the watchdog without loss. v3 fields the server adds later will reach
// the consumer (patched yt-dlp / mesh server) untouched.
using System.Text.Json;
using System.Text.Json.Serialization;

namespace VrcResolver.Shared;

public static class WireConstants
{
    public const string PipeName = "vrcresolver.resolve";

    // Wire shape this client speaks. Stamped on resolve requests' optional
    // protocol_version field once welcome confirms the server's negotiated
    // version. v3 adds permessage-deflate compression on the WS upgrade
    // (negotiated via Sec-WebSocket-Extensions) and a welcome-cache
    // handshake (client_hello → welcome_cached or welcome). v2 servers
    // negotiate down to 2 via the existing Math.Clamp on welcome.
    public const int ClientProtocolVersion = 3;

    // v3 WebSocket subprotocol literal — sent in Sec-WebSocket-Protocol on
    // the upgrade. Server echoes the same string on accept; if it comes
    // back as anything else the client falls back to the v2 path (skip
    // client_hello, wait for plain welcome). The server accepts both this
    // value and the pre-rename subprotocol string from older clients.
    public const string SubprotocolV3 = "vrcresolver-v3";

    // Action vocabulary
    public const string ActionResolve = "resolve";
    public const string ActionResolved = "resolved";
    public const string ActionFallbackNative = "fallback_native";
    public const string ActionResolveLog = "resolve_log";
    public const string ActionPing = "ping";
    public const string ActionPong = "pong";
    public const string ActionWelcome = "welcome";
    // v3: client → server first frame. Carries the optional welcome_hash
    // (sha256-prefix of a previously-cached welcome from this node) so
    // the server can skip resending unchanged welcome contents.
    public const string ActionClientHello = "client_hello";
    // v3: server → client when our welcome_hash matched. Carries only the
    // dynamic fields (warp_active, node label) — engines/features/version
    // strings are reused from the local cache keyed by the hash we sent.
    public const string ActionWelcomeCached = "welcome_cached";
    // Client → server feedback. Fire-and-forget; server must not respond
    // and must tolerate parse errors. See VrcLogMonitor for emission
    // semantics and PlaybackFeedbackKind for the value vocabulary.
    public const string ActionPlaybackFeedback = "playback_feedback";
    public const string PlaybackFeedbackLoadFailure = "load_failure";
    public const string PlaybackFeedbackSilentStall = "silent_stall";
    public const string PlaybackFeedbackPlaying = "playing";

    // v3.2: wrapper -> watchdog one-shot notification. The patched yt-dlp
    // wrapper sends one of these on a fresh pipe connection just before
    // it execs vanilla yt-dlp-og.exe (or emits empty stdout if og is
    // missing). Fire-and-forget on the wrapper side; the watchdog reads
    // it, emits a single console line, and writes nothing back. Reasons
    // mirror the wrapper's outcome vocabulary so the operator sees the
    // same labels in the watchdog console that appear in
    // yt-dlp-wrapper.log. NOT routed through the mesh -- this is a
    // local-only diagnostic channel.
    public const string ActionOgFallbackNotify = "og_fallback_notify";

    public const string OgFallbackReasonPipeConnectFailed = "pipe_connect_failed";
    public const string OgFallbackReasonPipeResolveFailed = "pipe_resolve_failed";
    public const string OgFallbackReasonServerFallbackNative = "server_fallback_native";
    public const string OgFallbackReasonNoUrlDiagnostic = "no_url_diagnostic";
    // VrcLogMonitor saw an AVPro load_failure on a URL we just resolved.
    // Within a short TTL after that observation, subsequent wrapper calls
    // for the same source URL short-circuit straight to native og so the
    // user doesn't have to repeatedly hit Play while the mesh re-tries
    // the same broken strategy.
    public const string OgFallbackReasonPriorLoadFailure = "prior_load_failure";
    // Defense-in-depth: server's FormatSelectorBuilder should already
    // filter AVPro-incompatible codecs, but a domain-config regression
    // could yield e.g. .flv / rtmp where AVPro can't decode. The wrapper
    // catches the obvious cases via URL shape (no network call) and
    // execs og instead of handing VRChat an unplayable URL.
    public const string OgFallbackReasonAvProIncompatible = "avpro_incompatible";
    // The resolved URL failed the emit-shape guard (non-http scheme, loopback
    // or private-range IP literal, localhost host). Never handed to VRChat;
    // the wrapper execs og instead. See ResolvedUrlGuard.
    public const string OgFallbackReasonResolvedUrlRejected = "resolved_url_rejected";
    // Watchdog-side health gate is open: recent resolves or playbacks failed
    // in a row, so resolve requests short-circuit straight to og until a
    // probe succeeds. Local pipe vocabulary only -- never sent to the server.
    public const string OgFallbackReasonResolverUnhealthy = "resolver_unhealthy";
    // og itself failed (CF 403 / 429 / sign-in-required). Wrapper sends
    // this notify on the same pipe channel as ActionOgFallbackNotify so
    // the watchdog can evict any stale cache entry for the URL and
    // surface a single user-visible "og also failed" line.
    public const string ActionWrapperOgFailedNotify = "wrapper_og_failed";

    // v2 field names (snake_case, mirror server constants verbatim)
    public const string FieldProtocolVersion = "protocol_version";
    public const string FieldAcceptProtocols = "accept_protocols";
    public const string FieldAcceptCodecs = "accept_codecs";
    public const string FieldMaxAudioChannels = "max_audio_channels";
    public const string FieldVrchatFormatArg = "vrchat_format_arg";
    public const string FieldCorrelationId = "correlation_id";
    public const string FieldDeliveredHeight = "delivered_height";
    // v3 field on welcome / client_hello.
    public const string FieldWelcomeHash = "welcome_hash";

    // v3.1 fields. accept_formats on client_hello is the client's
    // preference-ordered list of post-welcome wire formats the server may
    // pick from. negotiated_format on welcome / welcome_cached is the
    // server's choice for THIS connection — fixed at handshake time, not
    // per-frame. Control frames (welcome / welcome_cached / client_hello)
    // are always JSON-Text regardless of the negotiated format; only the
    // hot-path frames (resolved / fallback_native / resolve_log) honour
    // the negotiation.
    public const string FieldAcceptFormats = "accept_formats";
    public const string FieldNegotiatedFormat = "negotiated_format";
    public const string FormatJson = "json";
    public const string FormatMsgpack = "msgpack";

    // v3 advisory feature strings that may appear in welcome.features. The
    // client doesn't gate on them — feature presence reflects what the
    // server promises, not what the client must verify.
    public const string FeatureV3Compression = "v3_compression";
    public const string FeatureWelcomeHashAck = "welcome_hash_ack";
    // v3.1: server advertises this when it can speak msgpack on the hot
    // path. Client gates the binary-frame dispatch on the welcome's
    // negotiated_format == "msgpack" (server picks per-connection from
    // our accept_formats list); the feature string is informational.
    public const string FeatureMsgpackFormat = "msgpack_format";
    // v3.1 client preference order for post-welcome wire format. Sent
    // verbatim as the client_hello.accept_formats field. The first
    // element is the most-preferred format; server picks the first
    // element from this list that it supports. msgpack is preferred for
    // its measured 60% wire-size and 67% decode-time win on the hot
    // path (commit-2 benchmark below); json is the universal fallback
    // so a v2 server (or a v3.0 server that doesn't know the field)
    // still resolves cleanly.
    public static readonly string[] AcceptFormatsPreference = { FormatMsgpack, FormatJson };

    // Sentinel for the v3.0-style behaviour: explicit json-only opt-out.
    // Maintains v3.0 wire shape — no msgpack Binary frames will arrive
    // even if the server advertises msgpack_format. Used during the
    // v3.1 staged rollout: commit 1 ships the JSON-side wire fields
    // with AcceptFormats fixed at this list so the receive loop (which
    // doesn't yet know how to decode Binary) never sees one. Commit 2
    // flips the watchdog over to AcceptFormatsPreference simultaneously
    // with the binary-frame dispatch implementation.
    public static readonly string[] AcceptFormatsJsonOnly = { FormatJson };

    // Player vocabulary. Spec is case-sensitive; only "avpro" and "unity" are
    // valid on the wire. PlayerUnknown is a watchdog-internal tag for log
    // lines when the patched yt-dlp didn't supply a player — never sent.
    public const string PlayerAvPro = "avpro";
    public const string PlayerUnity = "unity";
    public const string PlayerUnknown = "unknown";

    // Fallback reasons (existing v1)
    public const string FallbackAllConfigsFailed = "all_configs_failed";
    public const string FallbackDomainBlocked = "domain_blocked";
    public const string FallbackExtractorUnsupported = "extractor_unsupported";
    public const string FallbackInternalError = "internal_error";
    public const string FallbackDiscoveryInProgress = "discovery_in_progress";
    public const string FallbackServerUnreachable = "server_unreachable";

    // v2 fallback reasons. Both must NOT trigger a native-yt-dlp retry on the
    // patched-binary side: native would hit the same wall.
    //   - unity_unsupported_format: server couldn't produce a Unity-playable
    //     stream from any candidate; user should try AVPro.
    //   - warp_down: server's WARP egress is unhealthy; transient — user can
    //     retry in a few seconds, or another node may be healthy.
    public const string ReasonUnityUnsupportedFormat = "unity_unsupported_format";
    public const string ReasonWarpDown = "warp_down";

    // Default request profiles (server applies these when client omits the
    // matching field). Mirrored here for client-side logging clarity.
    public static readonly string[] AvProAcceptProtocols = { "http", "hls", "dash" };
    public static readonly string[] UnityAcceptProtocols = { "http" };
    public static readonly string[] AvProAcceptCodecs =
        { "h264", "h265", "vp9", "av1", "aac", "opus", "mp3", "ac3", "eac3" };
    public static readonly string[] UnityAcceptCodecs = { "h264", "aac" };
    public const int AvProMaxAudioChannels = 8;
    public const int UnityMaxAudioChannels = 2;
}

public sealed class ResolveRequest
{
    // v1 fields (camelCase preserved for backward compat with the existing
    // patched yt-dlp). Server still requires these.
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionResolve;
    [JsonPropertyName("id")] public string Id { get; set; } = "";
    [JsonPropertyName("url")] public string Url { get; set; } = "";
    [JsonPropertyName("player")] public string? Player { get; set; }
    [JsonPropertyName("maxHeight")] public int? MaxHeight { get; set; }

    // v2 fields (snake_case). Optional — send only when the watchdog has
    // confirmed the server speaks v2 via the welcome frame, OR pass through
    // verbatim if the patched yt-dlp populated them on the pipe request.
    [JsonPropertyName("protocol_version")] public int? ProtocolVersion { get; set; }
    [JsonPropertyName("correlation_id")] public string? CorrelationId { get; set; }
    [JsonPropertyName("accept_protocols")] public string[]? AcceptProtocols { get; set; }
    [JsonPropertyName("accept_codecs")] public string[]? AcceptCodecs { get; set; }
    [JsonPropertyName("max_audio_channels")] public int? MaxAudioChannels { get; set; }
    [JsonPropertyName("vrchat_format_arg")] public string? VrchatFormatArg { get; set; }

    // v3.3: wrapper informs the server of how many ms remain before the
    // wrapper itself must exec og fallback (18s budget minus connection +
    // IPC setup). Server may extend its discovery wait up to ~60s when the
    // wrapper still has budget to wait. Null = absent = server uses its
    // own defaults. JsonIgnoreCondition.WhenWritingNull keeps old servers
    // unaffected when this field is not set by an old client.
    [JsonPropertyName("wrapper_deadline_ms"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? WrapperDeadlineMs { get; set; }

    // Forward-compat: any field name we don't statically know (e.g., a v3 field)
    // round-trips through the watchdog from pipe -> WS without loss. Both
    // serializer and deserializer respect this when reflecting over the type.
    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

public sealed class ResolveResponse
{
    [JsonPropertyName("action")] public string Action { get; set; } = "";
    [JsonPropertyName("id")] public string Id { get; set; } = "";
    [JsonPropertyName("url")] public string? Url { get; set; }
    [JsonPropertyName("engine")] public string? Engine { get; set; }
    [JsonPropertyName("config")] public string? Config { get; set; }
    [JsonPropertyName("reason")] public string? Reason { get; set; }
    [JsonPropertyName("message")] public string? Message { get; set; }

    // v2 fields. Server emits these only when the request's protocol_version
    // was >= 2. "unknown" is a valid value (server couldn't determine), not
    // an error — treat it as v1 (no constraint info).
    [JsonPropertyName("container")] public string? Container { get; set; }
    [JsonPropertyName("video_codec")] public string? VideoCodec { get; set; }
    [JsonPropertyName("audio_codec")] public string? AudioCodec { get; set; }
    [JsonPropertyName("protocol")] public string? Protocol { get; set; }
    [JsonPropertyName("audio_channels")] public int? AudioChannels { get; set; }
    [JsonPropertyName("bytes_estimate")] public long? BytesEstimate { get; set; }
    [JsonPropertyName("expires_at")] public string? ExpiresAt { get; set; }

    // Operator-facing one-liner surfaced via og_fallback_notify when the server says
    // fallback_native. Null when the server didn't attach a public_message. Old clients
    // ignored unknown fields via [JsonExtensionData]; this typed slot just lets the
    // wrapper read it without dipping into Extra.
    [JsonPropertyName("public_message")] public string? PublicMessage { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

// Welcome frame: server → client, once per WS connection, ~50ms after accept.
// Carries server capabilities so the client knows whether to use v2 fields.
// Required: action, protocol_version, node, engines, features.
// Optional: warp_active, yt_dlp_version, server_version.
// v3 added: welcome_hash (server-emitted SHA256-prefix fingerprint of this
// welcome's contents — client persists it per-node and offers it back on
// the next reconnect's client_hello so the server can skip resending
// unchanged contents). Backward compat on v2 servers: the field is just
// absent; v2 deserialize unaffected.
public sealed class WelcomeFrame
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionWelcome;
    [JsonPropertyName("protocol_version")] public int ProtocolVersion { get; set; }
    [JsonPropertyName("node")] public string? Node { get; set; }
    [JsonPropertyName("warp_active")] public bool? WarpActive { get; set; }
    [JsonPropertyName("engines")] public string[]? Engines { get; set; }
    [JsonPropertyName("features")] public string[]? Features { get; set; }
    [JsonPropertyName("yt_dlp_version")] public string? YtDlpVersion { get; set; }
    [JsonPropertyName("server_version")] public string? ServerVersion { get; set; }
    [JsonPropertyName("welcome_hash")] public string? WelcomeHash { get; set; }
    // v3.1: server's chosen post-welcome wire format. Picked from the
    // client_hello.accept_formats list at handshake time and fixed for
    // the connection's lifetime. Null/absent means "json" (v3.0
    // behaviour). Values: "json" or "msgpack".
    [JsonPropertyName("negotiated_format")] public string? NegotiatedFormat { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

// v3 client → server, FIRST FRAME on a v3-negotiated connection. Sent
// before any resolve. Optional welcome_hash carries an opaque
// SHA256-prefix fingerprint of a previously-cached welcome from THIS
// node; null means "no cache, send me the full welcome." client_id is
// the same per-process GUID already used by playback_feedback frames so
// the server can correlate the same client across resolve + telemetry.
public sealed class ClientHelloFrame
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionClientHello;
    [JsonPropertyName("welcome_hash")] public string? WelcomeHash { get; set; }
    [JsonPropertyName("client_id")] public string ClientId { get; set; } = "";
    // v3.1: preference-ordered list of post-welcome wire formats the
    // server may choose from. Null/absent means "v3.0-style hello, server
    // defaults to json." See WireConstants.AcceptFormatsPreference.
    [JsonPropertyName("accept_formats")] public string[]? AcceptFormats { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

// v3 server → client when our welcome_hash matched. Carries only the
// dynamic fields the server may want to update on every reconnect
// (warp_active is the primary case — node may rotate which tier is
// healthy in real time). Engines / features / version strings are
// reused from the local cache keyed by the hash we sent in client_hello.
public sealed class WelcomeCachedFrame
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionWelcomeCached;
    [JsonPropertyName("protocol_version")] public int ProtocolVersion { get; set; }
    [JsonPropertyName("node")] public string? Node { get; set; }
    [JsonPropertyName("warp_active")] public bool? WarpActive { get; set; }
    // v3.1: same semantics as WelcomeFrame.NegotiatedFormat — server's
    // chosen post-welcome wire format. Repeated on welcome_cached
    // because the format is per-connection, not cached: a returning
    // watchdog may have cached features but the format choice depends
    // on the just-sent client_hello.accept_formats and is fresh.
    [JsonPropertyName("negotiated_format")] public string? NegotiatedFormat { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

// Client -> server fire-and-forget telemetry frame. Emitted by
// VrcLogMonitor when AVPro fails to load a URL the resolver returned
// (load_failure within 10 s of Opening, or silent_stall after 12 s of
// nothing), or when VRChat reports the active playback resolution
// (kind=playing). Server attributes the signal to the (domain, config)
// the resolver picked so future resolves can use real playback feedback.
//
// Pre-AOT migration this was built as a Dictionary<string, object?>
// and serialized via reflection. Replaced with a typed DTO so the
// source-gen JsonSerializerContext can produce a static formatter for
// it. Field names + types preserve the wire-shape exactly; server
// parses field-by-field, tolerates correlation_id absent. The
// JsonIgnoreCondition.WhenWritingNull on correlation_id matches the
// previous Dictionary-based serialization which omitted the field
// entirely when null.
public sealed class PlaybackFeedbackFrame
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionPlaybackFeedback;
    [JsonPropertyName("url")] public string Url { get; set; } = "";
    [JsonPropertyName("kind")] public string Kind { get; set; } = "";
    [JsonPropertyName("timestamp")] public string Timestamp { get; set; } = "";
    [JsonPropertyName("ms_since_open")] public int MsSinceOpen { get; set; }
    [JsonPropertyName("client_id")] public string ClientId { get; set; } = "";
    [JsonPropertyName("correlation_id"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? CorrelationId { get; set; }
    [JsonPropertyName("delivered_height"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? DeliveredHeight { get; set; }
}

// Wrapper -> watchdog one-shot notification frame, sent on a fresh pipe
// connection just before the wrapper execs vanilla yt-dlp-og.exe (or
// emits empty stdout if og is missing). Fire-and-forget: the wrapper
// closes the pipe immediately after sending and does not wait for an
// ack. The watchdog reads it, emits a single console line surfacing the
// fallback to the operator, and writes nothing back.
//
// NOT routed through the mesh -- this is local-only diagnostic visibility
// so the user can tell at a glance when a resolve flopped over to og.
// The wrapper's own yt-dlp-wrapper.log carries the full breadcrumb;
// this DTO is the bridge that makes the same fact visible on the
// watchdog console.
public sealed class WrapperEventNotify
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionOgFallbackNotify;
    [JsonPropertyName("url")] public string? Url { get; set; }
    [JsonPropertyName("reason")] public string? Reason { get; set; }
    [JsonPropertyName("elapsed_ms")] public long ElapsedMs { get; set; }
    [JsonPropertyName("rid")] public string? Rid { get; set; }
    // Populated only for ActionWrapperOgFailedNotify; mirrors og's exit
    // code and a sanitized stderr preview so the watchdog can decide
    // whether to evict a stale cache entry and surface a clear console
    // line. Both fields are absent (default values) on og_fallback_notify.
    [JsonPropertyName("exit_code")] public int ExitCode { get; set; }
    [JsonPropertyName("error_preview")] public string? ErrorPreview { get; set; }
    // Carried from the server's fallback_native frame when present -- the human-friendly
    // one-liner explaining WHY the server punted (DRM-protected, geo-blocked, sign-in
    // required, etc.). Watchdog prints this as a separate yellow line above the standard
    // [wrapper] og fallback log so the user sees the rationale without grepping JSONL.
    [JsonPropertyName("public_message")] public string? PublicMessage { get; set; }
}
