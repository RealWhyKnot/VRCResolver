using System.Text.Json;
using System.Text.Json.Serialization;

namespace VrcResolver.Shared;

public static class WireConstants
{
    public const string PipeName = "vrcresolver.resolve";

    public const int ClientProtocolVersion = 3;

    public const string SubprotocolV3 = "vrcresolver-v3";

    public const string ActionResolve = "resolve";
    public const string ActionResolved = "resolved";
    public const string ActionFallbackNative = "fallback_native";
    public const string ActionResolveLog = "resolve_log";
    public const string ActionPing = "ping";
    public const string ActionPong = "pong";
    public const string ActionWelcome = "welcome";
    public const string ActionClientHello = "client_hello";
    public const string ActionWelcomeCached = "welcome_cached";
    public const string ActionPlaybackFeedback = "playback_feedback";
    public const string PlaybackFeedbackLoadFailure = "load_failure";
    public const string PlaybackFeedbackSilentStall = "silent_stall";
    public const string PlaybackFeedbackPlaying = "playing";

    public const string PlaybackFeedbackResolvedRejected = "resolved_rejected";
    public const string PlaybackFeedbackOgFailed = "og_failed";
    public const string PlaybackFeedbackCachePlay = "cache_play";

    public const string ActionProtocolError = "protocol_error";
    public const string ActionRateLimited = "rate_limited";
    public const string FieldRetryAfterSeconds = "retryAfterSeconds";
    public const string FieldMeshAction = "meshAction";

    public const string ActionOgFallbackNotify = "og_fallback_notify";

    public const string OgFallbackReasonPipeConnectFailed = "pipe_connect_failed";
    public const string OgFallbackReasonPipeResolveFailed = "pipe_resolve_failed";
    public const string OgFallbackReasonServerFallbackNative = "server_fallback_native";
    public const string ServerReasonValidationFailedPrefix = "validation_failed_";
    public const string OgFallbackReasonNoUrlDiagnostic = "no_url_diagnostic";
    public const string OgFallbackReasonPriorLoadFailure = "prior_load_failure";
    public const string OgFallbackReasonAvProIncompatible = "avpro_incompatible";
    public const string OgFallbackReasonResolvedUrlRejected = "resolved_url_rejected";
    public const string OgFallbackReasonResolverUnhealthy = "resolver_unhealthy";
    public const string ActionWrapperOgFailedNotify = "wrapper_og_failed";

    public const string FieldProtocolVersion = "protocol_version";
    public const string FieldAcceptProtocols = "accept_protocols";
    public const string FieldAcceptCodecs = "accept_codecs";
    public const string FieldMaxAudioChannels = "max_audio_channels";
    public const string FieldVrchatFormatArg = "vrchat_format_arg";
    public const string FieldCorrelationId = "correlation_id";
    public const string FieldDeliveredHeight = "delivered_height";
    public const string FieldWelcomeHash = "welcome_hash";

    public const string FieldAcceptFormats = "accept_formats";
    public const string FieldNegotiatedFormat = "negotiated_format";
    public const string FormatJson = "json";
    public const string FormatMsgpack = "msgpack";

    public const string FeatureWelcomeHosts = "welcome_hosts";
    public const string FeaturePlaybackFeedbackV2 = "playback_feedback_v2";
    public static readonly string[] AcceptFormatsPreference = { FormatMsgpack, FormatJson };

    public static readonly string[] AcceptFormatsJsonOnly = { FormatJson };

    public const string PlayerAvPro = "avpro";
    public const string PlayerUnity = "unity";
    public const string PlayerUnknown = "unknown";

    public const string FallbackAllConfigsFailed = "all_configs_failed";
    public const string FallbackExtractorUnsupported = "extractor_unsupported";
    public const string FallbackInternalError = "internal_error";
    public const string FallbackDiscoveryInProgress = "discovery_in_progress";
    public const string FallbackServerUnreachable = "server_unreachable";
    public const string FallbackClientDeadlineExceeded = "client_deadline_exceeded";

    public const string FallbackRateLimited = "rate_limited";
    public const string FallbackProtocolError = "protocol_error";

    public const string ReasonUnityUnsupportedFormat = "unity_unsupported_format";
    public const string ReasonWarpDown = "warp_down";

    public static readonly string[] AvProAcceptProtocols = { "http", "hls", "dash" };
    public static readonly string[] UnityAcceptProtocols = { "http" };
    public static readonly string[] AvProAcceptCodecs =
        { "h264", "h265", "vp9", "av1", "aac", "opus", "mp3", "ac3", "eac3" };
    public static readonly string[] UnityAcceptCodecs = { "h264", "aac" };
    public const int AvProMaxAudioChannels = 8;
    public const int UnityMaxAudioChannels = 2;

    public const int HighQualityMaxHeight = 2160;
    public const int DefaultMaxHeight = 1080;

    public static readonly string[] ExtensionBackedVideoCodecs = { "h265", "vp9", "av1" };

    public static string[] BuildAcceptCodecs(IReadOnlySet<string>? verifiedVideoCodecs)
    {
        var result = new List<string>(AvProAcceptCodecs.Length);
        foreach (var codec in AvProAcceptCodecs)
        {
            bool extensionBacked = Array.IndexOf(ExtensionBackedVideoCodecs, codec) >= 0;
            if (extensionBacked && (verifiedVideoCodecs == null || !verifiedVideoCodecs.Contains(codec)))
                continue;
            result.Add(codec);
        }
        return result.ToArray();
    }
}

public sealed class ResolveRequest
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionResolve;
    [JsonPropertyName("id")] public string Id { get; set; } = "";
    [JsonPropertyName("url")] public string Url { get; set; } = "";
    [JsonPropertyName("player")] public string? Player { get; set; }
    [JsonPropertyName("maxHeight")] public int? MaxHeight { get; set; }

    [JsonPropertyName("protocol_version")] public int? ProtocolVersion { get; set; }
    [JsonPropertyName("correlation_id")] public string? CorrelationId { get; set; }
    [JsonPropertyName("accept_protocols")] public string[]? AcceptProtocols { get; set; }
    [JsonPropertyName("accept_codecs")] public string[]? AcceptCodecs { get; set; }
    [JsonPropertyName("max_audio_channels")] public int? MaxAudioChannels { get; set; }
    [JsonPropertyName("vrchat_format_arg")] public string? VrchatFormatArg { get; set; }

    [JsonPropertyName("wrapper_deadline_ms"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? WrapperDeadlineMs { get; set; }

    [JsonPropertyName("prefer_highest"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? PreferHighest { get; set; }

    [JsonPropertyName("skip_native_hint"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? SkipNativeHint { get; set; }

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

    [JsonPropertyName("container")] public string? Container { get; set; }
    [JsonPropertyName("video_codec")] public string? VideoCodec { get; set; }
    [JsonPropertyName("audio_codec")] public string? AudioCodec { get; set; }
    [JsonPropertyName("protocol")] public string? Protocol { get; set; }
    [JsonPropertyName("audio_channels")] public int? AudioChannels { get; set; }
    [JsonPropertyName("bytes_estimate")] public long? BytesEstimate { get; set; }
    [JsonPropertyName("expires_at")] public string? ExpiresAt { get; set; }
    [JsonPropertyName("resolved_height")] public int? ResolvedHeight { get; set; }

    [JsonPropertyName("public_message")] public string? PublicMessage { get; set; }

    [JsonPropertyName("retry_after_ms")] public int? RetryAfterMs { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

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
    [JsonPropertyName("negotiated_format")] public string? NegotiatedFormat { get; set; }
    [JsonPropertyName("first_party_hosts")] public string[]? FirstPartyHosts { get; set; }
    [JsonPropertyName("playback_proxy_paths")] public string[]? PlaybackProxyPaths { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

public sealed class ClientHelloFrame
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionClientHello;
    [JsonPropertyName("welcome_hash")] public string? WelcomeHash { get; set; }
    [JsonPropertyName("client_id")] public string ClientId { get; set; } = "";
    [JsonPropertyName("accept_formats")] public string[]? AcceptFormats { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

public sealed class WelcomeCachedFrame
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionWelcomeCached;
    [JsonPropertyName("protocol_version")] public int ProtocolVersion { get; set; }
    [JsonPropertyName("node")] public string? Node { get; set; }
    [JsonPropertyName("warp_active")] public bool? WarpActive { get; set; }
    [JsonPropertyName("negotiated_format")] public string? NegotiatedFormat { get; set; }

    [JsonExtensionData] public Dictionary<string, JsonElement>? Extra { get; set; }
}

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
    [JsonPropertyName("detail"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Detail { get; set; }
}

public sealed class WrapperEventNotify
{
    [JsonPropertyName("action")] public string Action { get; set; } = WireConstants.ActionOgFallbackNotify;
    [JsonPropertyName("url")] public string? Url { get; set; }
    [JsonPropertyName("reason")] public string? Reason { get; set; }
    [JsonPropertyName("elapsed_ms")] public long ElapsedMs { get; set; }
    [JsonPropertyName("rid")] public string? Rid { get; set; }
    [JsonPropertyName("exit_code")] public int ExitCode { get; set; }
    [JsonPropertyName("error_preview")] public string? ErrorPreview { get; set; }
    [JsonPropertyName("public_message")] public string? PublicMessage { get; set; }
}
