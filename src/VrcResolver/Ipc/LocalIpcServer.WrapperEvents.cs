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
        if (!string.IsNullOrEmpty(notify.PublicMessage))
            ConsoleUx.Warn(LogComponent.Wrapper, LogUtil.SanitizeForConsole(notify.PublicMessage, 160));
        ConsoleUx.WrapperFallback(host: host, reason: reason, elapsedMs: notify.ElapsedMs);
        Logger.WriteFileOnly(
            "[wrapper] og_fallback_notify rid=" + LogUtil.SanitizeForConsole(notify.Rid ?? "?", 16) +
            " host=" + host +
            " reason=" + reason +
            " elapsed_ms=" + notify.ElapsedMs);

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

        int evicted = 0;
        bool hintCleared = false;
        if (!string.IsNullOrEmpty(notify.Url))
        {
            try { evicted = _cache?.EvictByUrl(notify.Url) ?? 0; }
            catch { }
            hintCleared = _ogFallbackHint?.TryClear(notify.Url) ?? false;
        }

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
            " hint_cleared=" + hintCleared +
            " preview=" + preview);

        if (!string.IsNullOrEmpty(notify.Url))
        {
            string detail = OgFailedDetailFor(notify.Reason);
            _ = _mesh.SendPlaybackFeedbackAsync(notify.Url!,
                WireConstants.PlaybackFeedbackOgFailed, 0,
                detail: detail, correlationIdOverride: notify.Rid);
        }
    }

    internal static string OgFailedDetailFor(string? wrapperReason) => wrapperReason switch
    {
        "cf_403" or "rate_limited" or "sign_in_required" or "content_not_found" => wrapperReason,
        _ => "unknown",
    };

}
