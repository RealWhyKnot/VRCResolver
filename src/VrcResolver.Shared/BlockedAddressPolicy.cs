using System.Net;
using System.Net.Sockets;

namespace VrcResolver.Shared;

public static class BlockedAddressPolicy
{
    public static bool IsBlocked(IPAddress addr)
    {
        if (IPAddress.IsLoopback(addr)) return true;

        var bytes = addr.GetAddressBytes();
        switch (addr.AddressFamily)
        {
            case AddressFamily.InterNetwork:
                if (bytes[0] == 0) return true;
                if (bytes[0] == 10) return true;
                if (bytes[0] == 127) return true;
                if (bytes[0] == 169 && bytes[1] == 254) return true;
                if (bytes[0] == 172 && bytes[1] >= 16 && bytes[1] <= 31) return true;
                if (bytes[0] == 192 && bytes[1] == 168) return true;
                return false;

            case AddressFamily.InterNetworkV6:
                if (addr.IsIPv4MappedToIPv6)
                    return IsBlocked(addr.MapToIPv4());
                if ((bytes[0] & 0xFE) == 0xFC) return true;
                if (bytes[0] == 0xFE && (bytes[1] & 0xC0) == 0x80) return true;
                return false;

            default:
                return true;
        }
    }
}
