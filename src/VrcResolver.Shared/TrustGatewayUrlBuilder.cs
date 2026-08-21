using System.Globalization;
using System.Net;
using System.Text;

namespace VrcResolver.Shared;

public static class TrustGatewayUrlBuilder
{
    public static bool TryBuild(int port, string targetUrl, string? session, out string localUrl)
        => TryBuild(port, targetUrl, session, "http", out localUrl);

    public static bool TryBuild(int port, string targetUrl, string? session, string scheme, out string localUrl)
    {
        localUrl = "";
        if (port <= 1024 || port >= 65536) return false;
        if (!IsAllowedGatewayScheme(scheme)) return false;
        if (string.IsNullOrWhiteSpace(targetUrl)) return false;
        if (IsLocalTrustGatewayUrl(targetUrl)) return false;
        if (!FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUrl(targetUrl)) return false;

        string effectiveSession = string.IsNullOrWhiteSpace(session)
            ? Guid.NewGuid().ToString("N")[..12]
            : SanitizeSession(session);
        if (effectiveSession.Length == 0) return false;

        string encoded = Base64UrlText.Encode(targetUrl);
        string ext = FirstPartyUrlPolicy.PlaybackProxyExtensionForTrustGateway(targetUrl);
        string suffix = string.IsNullOrEmpty(ext) ? "" : "." + ext;

        localUrl = scheme.ToLowerInvariant() + "://localhost.youtube.com:"
            + port.ToString(CultureInfo.InvariantCulture)
            + "/play/" + effectiveSession + "/manifest" + suffix
            + "?target=" + encoded;
        return true;
    }

    public static bool TryExtractTarget(string localUrl, out string targetUrl)
    {
        targetUrl = "";
        if (string.IsNullOrWhiteSpace(localUrl)) return false;
        if (!Uri.TryCreate(localUrl, UriKind.Absolute, out var uri)) return false;
        if (!IsLocalTrustGatewayHost(uri.Host)) return false;
        if (!uri.AbsolutePath.StartsWith("/play/", StringComparison.OrdinalIgnoreCase)) return false;

        string? encoded = FindQueryValue(uri.Query, "target");
        if (string.IsNullOrWhiteSpace(encoded)) return false;

        if (!Base64UrlText.TryDecode(encoded, out string decoded)) return false;
        if (!Uri.TryCreate(decoded, UriKind.Absolute, out _)) return false;
        targetUrl = decoded;
        return true;
    }

    private static string SanitizeSession(string session)
    {
        Span<char> buffer = session.Length <= 64
            ? stackalloc char[session.Length]
            : new char[64];
        int written = 0;
        foreach (char ch in session)
        {
            if (written >= buffer.Length) break;
            if ((ch >= 'a' && ch <= 'z')
                || (ch >= 'A' && ch <= 'Z')
                || (ch >= '0' && ch <= '9'))
            {
                buffer[written++] = ch;
            }
        }
        return written == 0 ? "" : new string(buffer[..written]);
    }

    public static bool IsLocalTrustGatewayHost(string host)
    {
        return host.Equals("localhost.youtube.com", StringComparison.OrdinalIgnoreCase)
            || host.Equals("localhost", StringComparison.OrdinalIgnoreCase)
            || host.Equals("127.0.0.1", StringComparison.OrdinalIgnoreCase);
    }

    private static bool IsLocalTrustGatewayUrl(string url)
    {
        return Uri.TryCreate(url, UriKind.Absolute, out var uri)
            && IsLocalTrustGatewayHost(uri.Host);
    }

    public static bool IsAllowedGatewayScheme(string? scheme)
    {
        return string.Equals(scheme, "http", StringComparison.OrdinalIgnoreCase)
            || string.Equals(scheme, "https", StringComparison.OrdinalIgnoreCase);
    }

    private static string? FindQueryValue(string query, string name)
    {
        if (string.IsNullOrEmpty(query)) return null;
        ReadOnlySpan<char> remaining = query.AsSpan();
        if (remaining.Length > 0 && remaining[0] == '?') remaining = remaining[1..];

        while (!remaining.IsEmpty)
        {
            int amp = remaining.IndexOf('&');
            ReadOnlySpan<char> part = amp >= 0 ? remaining[..amp] : remaining;
            int eq = part.IndexOf('=');
            if (eq > 0
                && part[..eq].Equals(name.AsSpan(), StringComparison.OrdinalIgnoreCase))
            {
                return WebUtility.UrlDecode(part[(eq + 1)..].ToString());
            }

            if (amp < 0) break;
            remaining = remaining[(amp + 1)..];
        }

        return null;
    }
}
