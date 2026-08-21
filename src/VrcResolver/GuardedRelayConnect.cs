using System.Net;
using System.Net.Sockets;
using VrcResolver.Shared;

namespace VrcResolver;

// LocalRelaySecurity validates the target URL the caller submitted, but the
// relay's HttpClient follows redirects, so a validated first-party URL that
// 302s to 127.0.0.1 / 169.254.169.254 / an RFC1918 host would be fetched
// anyway -- and even without a redirect there is a DNS-rebinding window
// between the allowlist check and the socket's own lookup. This connect
// callback runs at every actual TCP connect, including each redirect hop, and
// refuses a socket to a blocked address. Upstream targets are public
// first-party hosts, so a blocked answer here is never legitimate playback.
internal static class GuardedRelayConnect
{
    public static Func<SocketsHttpConnectionContext, CancellationToken, ValueTask<Stream>> Callback { get; }
        = (context, ct) => ConnectAsync(context.DnsEndPoint, ct);

    internal static async ValueTask<Stream> ConnectAsync(DnsEndPoint dns, CancellationToken ct)
    {
        IPAddress[] addresses = IPAddress.TryParse(dns.Host, out var literal)
            ? new[] { literal }
            : await Dns.GetHostAddressesAsync(dns.Host, ct).ConfigureAwait(false);

        // Any blocked address in the set fails the connect: this is what
        // defeats a rebinding resolver that returns one public and one
        // internal answer.
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
