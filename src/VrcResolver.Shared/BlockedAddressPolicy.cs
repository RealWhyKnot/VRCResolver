using System.Net;
using System.Net.Sockets;

namespace VrcResolver.Shared;

// Address ranges nothing on the client should fetch or hand to VRChat:
// loopback, RFC1918 LAN, link-local (incl. cloud metadata at 169.254.169.254),
// and their IPv6 equivalents. Kept in sync with the server-side guard so both
// halves answer alike.
public static class BlockedAddressPolicy
{
    public static bool IsBlocked(IPAddress addr)
    {
        if (IPAddress.IsLoopback(addr)) return true;

        var bytes = addr.GetAddressBytes();
        switch (addr.AddressFamily)
        {
            case AddressFamily.InterNetwork:
                // 0.0.0.0/8 -- "this network" / unspecified
                if (bytes[0] == 0) return true;
                // 10.0.0.0/8
                if (bytes[0] == 10) return true;
                // 127.0.0.0/8
                if (bytes[0] == 127) return true;
                // 169.254.0.0/16 -- link-local + cloud metadata
                if (bytes[0] == 169 && bytes[1] == 254) return true;
                // 172.16.0.0/12
                if (bytes[0] == 172 && bytes[1] >= 16 && bytes[1] <= 31) return true;
                // 192.168.0.0/16
                if (bytes[0] == 192 && bytes[1] == 168) return true;
                return false;

            case AddressFamily.InterNetworkV6:
                // IPv4-mapped IPv6 (::ffff:a.b.c.d) -- unwrap and recheck against
                // the v4 ranges so a mapped address can't smuggle a private v4
                // past the v6 path.
                if (addr.IsIPv4MappedToIPv6)
                    return IsBlocked(addr.MapToIPv4());
                // fc00::/7 -- Unique Local Addresses (RFC 4193)
                if ((bytes[0] & 0xFE) == 0xFC) return true;
                // fe80::/10 -- Link-Local
                if (bytes[0] == 0xFE && (bytes[1] & 0xC0) == 0x80) return true;
                return false;

            default:
                // Unknown address family -- block conservatively
                return true;
        }
    }
}
