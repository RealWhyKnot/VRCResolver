using System.Text;

namespace VrcResolver.Shared;

// Small helpers for safe console output. Used wherever a server- or
// patched-yt-dlp-supplied string (action name, URL, exception message)
// might reach Console.WriteLine. Goals:
//   - Strip control characters so an attacker can't smuggle ANSI escape
//     sequences into the user's console window (set title, scroll, hide
//     cursor, etc.).
//   - Cap length so a giant payload can't fill the scrollback.
//   - Redact short-lived tokens from URLs that may end up in bug reports.
public static class LogUtil
{
    // Replace ASCII control characters (< 0x20, 0x7F) and Unicode line/
    // paragraph separators with '?'. Truncate to maxLen with a trailing
    // ellipsis if the input was longer.
    public static string SanitizeForConsole(string? value, int maxLen = 120)
    {
        if (string.IsNullOrEmpty(value)) return value ?? "";
        if (maxLen < 1) maxLen = 1;

        var sb = new StringBuilder(Math.Min(value.Length, maxLen) + 1);
        int taken = 0;
        for (int i = 0; i < value.Length && taken < maxLen; i++)
        {
            char c = value[i];
            // U+2028 (LINE SEPARATOR) and U+2029 (PARAGRAPH SEPARATOR) are
            // both treated as control-character equivalents by terminals.
            if (c < 0x20 || c == 0x7F || c == 0x2028 || c == 0x2029)
                sb.Append('?');
            else
                sb.Append(c);
            taken++;
        }
        if (value.Length > maxLen) sb.Append("...");
        return sb.ToString();
    }

    // Strip query string from a URL so signed-URL tokens (S3 presigned,
    // Cloudfront, googlevideo signatures) don't end up in logs / bug reports.
    // Returns scheme+host+path; falls back to a sanitized version of the input
    // if it's not a valid URL.
    public static string RedactUrl(string? url)
    {
        if (string.IsNullOrEmpty(url)) return url ?? "";
        if (Uri.TryCreate(url, UriKind.Absolute, out var u))
        {
            string path = u.AbsolutePath.Length > 60
                ? u.AbsolutePath.Substring(0, 60) + "..."
                : u.AbsolutePath;
            return u.Scheme + "://" + u.Host + path;
        }
        return SanitizeForConsole(url, 120);
    }

    // Bare host (leading "www." stripped) or "?" -- for log lines where the
    // full URL would carry user-identifying tokens (video ids, stream names).
    public static string BareHost(string? url)
    {
        if (string.IsNullOrEmpty(url)) return "?";
        try
        {
            if (Uri.TryCreate(url, UriKind.Absolute, out var u))
            {
                string h = u.Host;
                if (h.StartsWith("www.", StringComparison.OrdinalIgnoreCase)) h = h[4..];
                return h;
            }
        }
        catch { /* best-effort */ }
        return "?";
    }

    // First N UTF-8 bytes of a payload, sanitized for console display.
    // Useful for "this frame failed to parse - here's what it looked like".
    public static string PayloadPreview(byte[] payload, int maxBytes = 120)
    {
        if (payload == null || payload.Length == 0) return "";
        int len = Math.Min(payload.Length, maxBytes);
        string s;
        try { s = Encoding.UTF8.GetString(payload, 0, len); }
        catch { return "<unparseable>"; }
        return SanitizeForConsole(s, maxBytes);
    }
}
