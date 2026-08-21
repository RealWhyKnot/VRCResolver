using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class LocalIpcServer : IDisposable
{
    private void HandleWrapperEvent(WrapperEventNotify notify)
    {
        if (string.Equals(notify.Action, WireConstants.ActionWrapperOgFailedNotify, StringComparison.Ordinal))
        {
            HandleOgFailedNotify(notify);
            return;
        }
        HandleOgFallbackNotify(notify);
    }
    // Cheap pre-deserialize peek: is this one of the wrapper's notify
    // frames (og_fallback_notify or wrapper_og_failed)? Avoids parsing
    // as ResolveRequest first (which would drop the unrecognized fields
    // into [JsonExtensionData] instead of routing to the dispatch).
    private static bool LooksLikeWrapperEventNotify(string line)
    {
        int probeLen = Math.Min(line.Length, 256);
        var head = line.AsSpan(0, probeLen);
        return head.IndexOf("og_fallback_notify".AsSpan(), StringComparison.Ordinal) >= 0
            || head.IndexOf("wrapper_og_failed".AsSpan(), StringComparison.Ordinal) >= 0;
    }

    private void HandleOgFallbackNotify(WrapperEventNotify notify)
    {
        string host = string.IsNullOrEmpty(notify.Url) ? "<no-url>" : LogUtil.BareHost(notify.Url);
        string reason = LogUtil.SanitizeForConsole(notify.Reason ?? "?", 32);
        // The server's own one-liner for WHY it punted (DRM, geo-block, sign-in
        // gate...). Above the fallback line so the user reads cause before effect.
        if (!string.IsNullOrEmpty(notify.PublicMessage))
            ConsoleUx.Warn(LogComponent.Wrapper, LogUtil.SanitizeForConsole(notify.PublicMessage, 160));
        // Pairs visually with the !! fallback colour on the resolve summary
        // line -- the wrapper's og fallback path is the same outcome category.
        ConsoleUx.WrapperFallback(host: host, reason: reason, elapsedMs: notify.ElapsedMs);
        Logger.WriteFileOnly(
            "[wrapper] og_fallback_notify rid=" + LogUtil.SanitizeForConsole(notify.Rid ?? "?", 16) +
            " host=" + host +
            " reason=" + reason +
            " elapsed_ms=" + notify.ElapsedMs);

        // The wrapper refused OUR resolved URL: that is a config failure the
        // server can score, and it cannot see it any other way. Feature-gated
        // inside SendPlaybackFeedbackAsync; fire-and-forget.
        if (!string.IsNullOrEmpty(notify.Url)
            && (notify.Reason == WireConstants.OgFallbackReasonResolvedUrlRejected
                || notify.Reason == WireConstants.OgFallbackReasonAvProIncompatible))
        {
            _ = _mesh.SendPlaybackFeedbackAsync(notify.Url!,
                WireConstants.PlaybackFeedbackResolvedRejected, 0,
                detail: notify.Reason, correlationIdOverride: notify.Rid);
        }
    }

    private void HandleOgFailedNotify(WrapperEventNotify notify)
    {
        string host = string.IsNullOrEmpty(notify.Url) ? "<no-url>" : LogUtil.BareHost(notify.Url);
        string reason = LogUtil.SanitizeForConsole(notify.Reason ?? "?", 32);
        string preview = LogUtil.SanitizeForConsole(notify.ErrorPreview ?? "", 80);

        // Evict any cached resolve for this URL -- the cache may have held
        // an entry from before the upstream blocker (CF challenge, sign-in
        // gate) appeared. Next VRChat retry for the same URL will skip the
        // cache and re-hit the mesh, which by then may have completed
        // discovery_in_progress or chosen a different strategy.
        int evicted = 0;
        if (!string.IsNullOrEmpty(notify.Url))
        {
            try { evicted = _cache?.EvictByUrl(notify.Url) ?? 0; }
            catch { /* best-effort */ }
        }

        // Short human hint after the machine-readable token. Keeps the token in
        // the line for grep/log triage while making the cause obvious to a user
        // glancing at the console. Unknown stays bare so the line doesn't lie
        // about what we know.
        string hint = reason switch
        {
            "content_not_found" => " (video unavailable upstream)",
            "cf_403" => " (403 blocked)",
            "rate_limited" => " (rate limited)",
            "sign_in_required" => " (auth gate)",
            _ => "",
        };
        ConsoleUx.Warn(
            LogComponent.Wrapper,
            "!! og also failed " + host + " reason=" + reason + " exit=" + notify.ExitCode + hint);
        Logger.WriteFileOnly(
            "[wrapper] wrapper_og_failed rid=" + LogUtil.SanitizeForConsole(notify.Rid ?? "?", 16) +
            " host=" + host +
            " reason=" + reason +
            " exit=" + notify.ExitCode +
            " elapsed_ms=" + notify.ElapsedMs +
            " evicted=" + evicted +
            " preview=" + preview);

        // Upstream-environment signal for the server (never scored there):
        // og hit the same wall we did, classified by the wrapper's og-stderr
        // vocabulary -- which is exactly the server's og_failed detail set.
        if (!string.IsNullOrEmpty(notify.Url))
        {
            string detail = OgFailedDetailFor(notify.Reason);
            _ = _mesh.SendPlaybackFeedbackAsync(notify.Url!,
                WireConstants.PlaybackFeedbackOgFailed, 0,
                detail: detail, correlationIdOverride: notify.Rid);
        }
    }

    // The server validates og_failed detail against a closed set; anything the
    // wrapper's classifier didn't recognize collapses to "unknown".
    internal static string OgFailedDetailFor(string? wrapperReason) => wrapperReason switch
    {
        "cf_403" or "rate_limited" or "sign_in_required" or "content_not_found" => wrapperReason,
        _ => "unknown",
    };

}
