using System.Text;

namespace VrcResolver.Shared;

public static class LogUtil
{
    public static string SanitizeForConsole(string? value, int maxLen = 120)
    {
        if (string.IsNullOrEmpty(value)) return value ?? "";
        if (maxLen < 1) maxLen = 1;

        var sb = new StringBuilder(Math.Min(value.Length, maxLen) + 1);
        int taken = 0;
        for (int i = 0; i < value.Length && taken < maxLen; i++)
        {
            char c = value[i];
            if (c < 0x20 || c == 0x7F || c == 0x2028 || c == 0x2029)
                sb.Append('?');
            else
                sb.Append(c);
            taken++;
        }
        if (value.Length > maxLen) sb.Append("...");
        return sb.ToString();
    }

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
        catch { }
        return "?";
    }

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
