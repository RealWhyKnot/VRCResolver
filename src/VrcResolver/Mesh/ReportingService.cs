using System.Net.Http;
using System.Net.Http.Json;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using VrcResolver.Shared;

namespace VrcResolver;

// Anonymous failure reporting to ServerEndpoints.ReportUrl. Restored from
// the legacy ReportingService — opt-in via env var, privacy-scrubbed
// before send, rate-limited.
//
// Privacy contract (mirrors what the server validates):
//   - Full URL is NEVER transmitted. Only the bare domain (host minus
//     "www." prefix). Paths and query strings are reduced to a 12-char
//     hex hash of SHA-256.
//   - For YouTube specifically, the video ID is also hashed.
//   - Error strings (yt-dlp stderr, exception messages, mesh reason
//     codes) are run through Sanitize() which strips:
//       * %USERPROFILE% and the literal Environment.UserName
//       * Windows-style C:\... paths and Unix-style /home/... paths
//       * IPv4 literals
//       * the original URL (so we don't accidentally re-leak it via the
//         error text)
//       * long token-shaped sequences (>=20 contiguous base64 chars)
//   - Rate limit: at most 1 report per 30s, max 20 per process session.
//     The server applies its own per-IP rate limit on top.
//
// Opt-in: set VRCRESOLVER_ANONYMOUS_REPORTING=1 at process launch (the
// pre-rename env prefix keeps working via LegacyCompat). Default OFF.
internal static partial class ReportingService
{
    private static readonly TimeSpan MinInterval = TimeSpan.FromSeconds(30);
    private const int MaxPerSession = 20;

    private static readonly object _gate = new();
    private static readonly HttpClient _http = new() { Timeout = TimeSpan.FromSeconds(20) };
    private static bool _enabled;
    private static int _sentThisSession;
    private static DateTime _lastSendUtc = DateTime.MinValue;

    public static bool Enabled => _enabled;

    static ReportingService()
    {
        var asmVer = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "0.0.0.0";
        _http.DefaultRequestHeaders.UserAgent.ParseAdd("VRCResolver-Reporter/" + asmVer);
    }

    public static void Initialize()
    {
        try
        {
            _enabled = string.Equals(
                LegacyCompat.GetEnvWithLegacyFallback("ANONYMOUS_REPORTING"),
                "1",
                StringComparison.Ordinal);
        }
        catch { _enabled = false; }

        if (_enabled)
            ConsoleUx.Write(LogComponent.Report, "anonymous failure reporting ON (VRCRESOLVER_ANONYMOUS_REPORTING=1)");
    }

    // Fire a report when the mesh returns fallback_native. Filters out
    // transient reasons that aren't failure-of-strategy (server_unreachable,
    // discovery_in_progress) — those would just spam the channel during
    // a network outage.
    public static void ReportFallback(ResolveRequest req, string reason, string? errorSummary)
    {
        if (!_enabled || req == null || string.IsNullOrEmpty(req.Url)) return;
        string failureKind = MapReasonToFailureKind(reason);
        if (string.IsNullOrEmpty(failureKind)) return; // skipped (transient)
        _ = Task.Run(() => SendAsync(req, failureKind, errorSummary));
    }

    private static async Task SendAsync(ResolveRequest req, string failureKind, string? errorSummary)
    {
        // Rate-limit gate.
        lock (_gate)
        {
            if (_sentThisSession >= MaxPerSession) return;
            if (DateTime.UtcNow - _lastSendUtc < MinInterval) return;
            _sentThisSession++;
            _lastSendUtc = DateTime.UtcNow;
        }

        string? domain = ExtractDomain(req.Url);
        if (string.IsNullOrEmpty(domain)) return;

        string player = req.Player == WireConstants.PlayerUnity
            ? WireConstants.PlayerUnity
            : WireConstants.PlayerAvPro;

        var payload = new ReportPayload
        {
            AppVersion = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "0.0.0.0",
            FailureKind = failureKind,
            UrlDomain = domain,
            Player = player,
            StreamType = "vod", // we have no reliable signal client-side; server budgets are loose
            UrlPathHashShort = HashShort(ExtractPathAndQuery(req.Url)),
            VideoIdHashShort = HashShort(ExtractYouTubeVideoId(req.Url) ?? ""),
            ErrorSummary = TrimTo(Sanitize(errorSummary ?? "", req.Url), 500),
        };

        try
        {
            // AOT migration: typed PostAsJsonAsync overload routed through
            // MeshJsonContext source-gen. Equivalent wire output to the
            // pre-AOT reflection-based extension method; just AOT-clean.
            using var resp = await _http.PostAsJsonAsync(
                ServerEndpoints.ReportUrl, payload, MeshJsonContext.Default.ReportPayload).ConfigureAwait(false);
            if (!resp.IsSuccessStatusCode)
                ConsoleUx.Warn(LogComponent.Report, "server rejected report: " + (int)resp.StatusCode);
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Report, "post failed: " + ex.GetType().Name + ": " + ex.Message);
        }
    }

    private static string MapReasonToFailureKind(string reason)
    {
        // Mesh fallback reasons → server's AllowedFailureKinds set
        // {Unknown, NetworkError, Timeout, Blocked403, NotFound404,
        //  JsChallenge, LowQuality, PlaybackFailed, AllStrategiesFailed}.
        return reason switch
        {
            WireConstants.FallbackAllConfigsFailed => "AllStrategiesFailed",
            WireConstants.FallbackExtractorUnsupported => "Unknown",
            WireConstants.FallbackInternalError => "Unknown",
            WireConstants.ReasonUnityUnsupportedFormat => "Unknown",
            WireConstants.ReasonWarpDown => "NetworkError",
            // Transient or server-refused — don't report:
            WireConstants.FallbackServerUnreachable => "",
            WireConstants.FallbackDiscoveryInProgress => "",
            WireConstants.FallbackRateLimited => "",
            WireConstants.FallbackProtocolError => "",
            _ => "",
        };
    }

    // Returns the bare host (no www., no port, lowercase).
    private static string? ExtractDomain(string url)
    {
        try
        {
            if (!Uri.TryCreate(url, UriKind.Absolute, out var u)) return null;
            string host = u.Host.ToLowerInvariant();
            if (host.StartsWith("www.")) host = host[4..];
            return host;
        }
        catch { return null; }
    }

    private static string ExtractPathAndQuery(string url)
    {
        try
        {
            if (!Uri.TryCreate(url, UriKind.Absolute, out var u)) return "";
            return u.PathAndQuery;
        }
        catch { return ""; }
    }

    [GeneratedRegex(@"(?:v=|/embed/|/v/|/live/|youtu\.be/)([A-Za-z0-9_-]{11})")]
    private static partial Regex YouTubeIdRegex();

    private static string? ExtractYouTubeVideoId(string url)
    {
        var m = YouTubeIdRegex().Match(url);
        return m.Success ? m.Groups[1].Value : null;
    }

    // SHA-256 prefix — first 12 hex chars. Stable across reports for the
    // same value so the server-side dashboard can correlate, but
    // computationally one-way so the original input isn't recoverable.
    private static string HashShort(string s)
    {
        if (string.IsNullOrEmpty(s)) return "";
        Span<byte> hash = stackalloc byte[32];
        SHA256.HashData(Encoding.UTF8.GetBytes(s), hash);
        var sb = new StringBuilder(12);
        for (int i = 0; i < 6; i++) sb.Append(hash[i].ToString("x2"));
        return sb.ToString();
    }

    private static string TrimTo(string s, int maxLen) =>
        s.Length <= maxLen ? s : s[..maxLen];

    // Strip user-identifying tokens and the original URL from a free-form
    // string before it's sent. The server rejects on a leak-pattern hit
    // anyway; this is the client-side first pass so we don't waste
    // round-trips on rejected payloads.
    [GeneratedRegex(@"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b")]
    private static partial Regex Ipv4();
    [GeneratedRegex(@"[A-Za-z]:\\[^\s""]+")]
    private static partial Regex WindowsPath();
    [GeneratedRegex(@"/(?:home|Users|root)/[^\s""]+")]
    private static partial Regex UnixPath();
    [GeneratedRegex(@"[A-Za-z0-9+/=_\-]{20,}")]
    private static partial Regex LongToken();

    public static string Sanitize(string s, string? originalUrl = null)
    {
        if (string.IsNullOrEmpty(s)) return s;
        try
        {
            string un = Environment.UserName;
            if (!string.IsNullOrEmpty(un) && un.Length >= 3)
                s = s.Replace(un, "<user>", StringComparison.OrdinalIgnoreCase);
        }
        catch { /* best-effort */ }
        try
        {
            string up = Environment.GetEnvironmentVariable("USERPROFILE") ?? "";
            if (!string.IsNullOrEmpty(up))
                s = s.Replace(up, "%USERPROFILE%", StringComparison.OrdinalIgnoreCase);
        }
        catch { /* best-effort */ }
        if (!string.IsNullOrEmpty(originalUrl))
            s = s.Replace(originalUrl, "<url>", StringComparison.OrdinalIgnoreCase);
        try { s = Environment.MachineName is { Length: > 0 } mn ? s.Replace(mn, "<machine>", StringComparison.OrdinalIgnoreCase) : s; }
        catch { /* best-effort */ }
        s = WindowsPath().Replace(s, "<path>");
        s = UnixPath().Replace(s, "<path>");
        s = Ipv4().Replace(s, "<ip>");
        s = LongToken().Replace(s, "<token>");
        return s;
    }

    // AOT migration: promoted private -> internal so MeshJsonContext can
    // [JsonSerializable(typeof(ReportingService.ReportPayload))] and emit
    // a source-gen formatter the typed PostAsJsonAsync overload uses.
    internal sealed class ReportPayload
    {
        [JsonPropertyName("appVersion")] public string AppVersion { get; set; } = "";
        [JsonPropertyName("failureKind")] public string FailureKind { get; set; } = "";
        [JsonPropertyName("urlDomain")] public string UrlDomain { get; set; } = "";
        [JsonPropertyName("player")] public string Player { get; set; } = "";
        [JsonPropertyName("streamType")] public string StreamType { get; set; } = "";
        [JsonPropertyName("urlPathHashShort")] public string? UrlPathHashShort { get; set; }
        [JsonPropertyName("videoIdHashShort")] public string? VideoIdHashShort { get; set; }
        [JsonPropertyName("errorSummary")] public string? ErrorSummary { get; set; }
    }
}
