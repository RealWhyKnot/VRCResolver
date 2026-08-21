using System.Net;

namespace VrcResolver.Shared;

// Last gate before a resolved URL is written to VRChat's stdout. The server --
// or a tampered resolve_cache.json, since LocalLow is writable at Low
// integrity -- could hand back a URL pointing at loopback, the LAN, or a
// non-http scheme, and VRChat would fetch it blindly. Shape checks only: no
// DNS lookups here, this runs on the wrapper's hot path. Public IP literals
// and DNS names pass; VRChat does the fetching, this guard exists to stop
// poisoned resolved URLs, not to police the public internet.
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

        // localhost / *.localhost resolve to loopback by definition. Our own
        // trust-gateway host (localhost.youtube.com) is minted locally AFTER
        // this guard runs, so it is never the URL being checked here.
        if (uri.Host.Equals("localhost", StringComparison.OrdinalIgnoreCase)
            || uri.Host.EndsWith(".localhost", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        return true;
    }
}
