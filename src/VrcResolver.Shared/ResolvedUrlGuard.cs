using System.Net;

namespace VrcResolver.Shared;

public static class ResolvedUrlGuard
{
    public static bool IsSafeToEmit(string? url)
    {
        if (string.IsNullOrWhiteSpace(url)) return false;
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri)) return false;
        if (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps) return false;
        if (uri.Host.Length == 0) return false;

        if (uri.HostNameType == UriHostNameType.IPv4 || uri.HostNameType == UriHostNameType.IPv6)
            return IPAddress.TryParse(uri.IdnHost, out var addr) && !BlockedAddressPolicy.IsBlocked(addr);

        if (uri.Host.Equals("localhost", StringComparison.OrdinalIgnoreCase)
            || uri.Host.EndsWith(".localhost", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        return true;
    }

    public static bool IsAvProCompatibleUrl(string url)
    {
        if (string.IsNullOrEmpty(url)) return false;
        string lower = url.ToLowerInvariant();
        if (lower.StartsWith("rtmp://", StringComparison.Ordinal)
            || lower.StartsWith("rtmps://", StringComparison.Ordinal)) return false;
        int q = lower.IndexOf('?');
        string pathLower = q >= 0 ? lower.Substring(0, q) : lower;
        if (pathLower.EndsWith(".flv", StringComparison.Ordinal)
            || pathLower.EndsWith(".f4v", StringComparison.Ordinal)) return false;
        return true;
    }
}
