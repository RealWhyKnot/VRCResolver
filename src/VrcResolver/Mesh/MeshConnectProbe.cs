using System.Diagnostics;
using System.Net;
using System.Net.Sockets;

namespace VrcResolver;

internal sealed class MeshConnectProbe
{
    internal const string DnsPhase = "dns";
    internal const string TcpPhase = "tcp";
    internal const string UpgradePhase = "upgrade";

    private string _phase = DnsPhase;
    private long _dnsMs = -1;
    private long _tcpMs = -1;

    public string Phase => Volatile.Read(ref _phase);
    public long DnsMs => Interlocked.Read(ref _dnsMs);
    public long TcpMs => Interlocked.Read(ref _tcpMs);

    internal void Enter(string phase) => Volatile.Write(ref _phase, phase);

    internal void RecordDns(long ms) => Interlocked.Exchange(ref _dnsMs, ms);

    internal void RecordTcp(long ms) => Interlocked.Exchange(ref _tcpMs, ms);

    internal static SocketsHttpHandler CreateHandler(MeshConnectProbe probe)
        => new() { ConnectCallback = (context, ct) => probe.ConnectAsync(context.DnsEndPoint, ct) };

    private async ValueTask<Stream> ConnectAsync(DnsEndPoint endPoint, CancellationToken ct)
    {
        Enter(DnsPhase);
        long started = Stopwatch.GetTimestamp();
        IPAddress[] addresses = await Dns.GetHostAddressesAsync(endPoint.Host, ct).ConfigureAwait(false);
        RecordDns(ElapsedMs(started));

        Enter(TcpPhase);
        started = Stopwatch.GetTimestamp();
        var socket = new Socket(SocketType.Stream, ProtocolType.Tcp) { NoDelay = true };
        try
        {
            await socket.ConnectAsync(addresses, endPoint.Port, ct).ConfigureAwait(false);
            RecordTcp(ElapsedMs(started));
            Enter(UpgradePhase);
            return new NetworkStream(socket, ownsSocket: true);
        }
        catch
        {
            socket.Dispose();
            throw;
        }
    }

    private static long ElapsedMs(long startedTimestamp)
        => (long)Stopwatch.GetElapsedTime(startedTimestamp).TotalMilliseconds;
}
