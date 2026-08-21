using System.Net.Sockets;

namespace VrcResolver.Shared;

// Bounded probe used by the wrapper before minting a trust-gateway URL from a
// port file that may have outlived the relay (watchdog killed, stale LocalLow
// state). Handing VRChat a localhost URL nothing is listening on produces a
// silent black player; emitting the raw first-party URL instead still plays
// in worlds that allow it.
public static class RelayLiveness
{
    public static bool IsListening(int port, int timeoutMs = 250)
    {
        if (port <= 0 || port >= 65536) return false;
        try
        {
            using var client = new TcpClient();
            var connect = client.ConnectAsync("127.0.0.1", port);
            bool ok = connect.Wait(timeoutMs) && client.Connected;
            // A timed-out connect faults after disposal; observe it off-path.
            if (!ok) connect.ContinueWith(static t => _ = t.Exception, TaskScheduler.Default);
            return ok;
        }
        catch
        {
            return false;
        }
    }
}
