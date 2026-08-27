using System.Net;
using System.Net.Sockets;
using VrcResolver.Shared;

namespace VrcResolver;

internal static class GuardedRelayConnect
{
    public static Func<SocketsHttpConnectionContext, CancellationToken, ValueTask<Stream>> Callback { get; }
        = (context, ct) => ConnectAsync(context.DnsEndPoint, ct);

    internal static async ValueTask<Stream> ConnectAsync(DnsEndPoint dns, CancellationToken ct)
    {
        IPAddress[] addresses = IPAddress.TryParse(dns.Host, out var literal)
            ? new[] { literal }
            : await Dns.GetHostAddressesAsync(dns.Host, ct).ConfigureAwait(false);

        foreach (var addr in addresses)
        {
            if (BlockedAddressPolicy.IsBlocked(addr))
                throw new HttpRequestException(
                    "Refused connection to a blocked internal address (" + addr + ").");
        }

        var socket = new Socket(SocketType.Stream, ProtocolType.Tcp) { NoDelay = true };
        try
        {
            await socket.ConnectAsync(addresses, dns.Port, ct).ConfigureAwait(false);
            return new NetworkStream(socket, ownsSocket: true);
        }
        catch
        {
            socket.Dispose();
            throw;
        }
    }
}
