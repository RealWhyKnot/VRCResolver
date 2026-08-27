using System.Net.Sockets;

namespace VrcResolver.Shared;

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
            if (!ok) connect.ContinueWith(static t => _ = t.Exception, TaskScheduler.Default);
            return ok;
        }
        catch
        {
            return false;
        }
    }
}
