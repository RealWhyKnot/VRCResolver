using System;
using System.IO;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
internal static class IpcPipeHarness
{
    public static async Task<string> RoundTripAsync(LocalIpcServer server, string pipeName, string requestLine, TimeSpan timeout)
    {
        server.StartForTests(pipeName);
        try
        {
            using var client = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
            await client.ConnectAsync(TimeSpan.FromSeconds(5), CancellationToken.None);
            await client.WriteAsync(Encoding.UTF8.GetBytes(requestLine + "\n"));

            using var reader = new StreamReader(client, Encoding.UTF8, leaveOpen: true);
            string? line = await reader.ReadLineAsync().WaitAsync(timeout);
            Assert.False(string.IsNullOrEmpty(line));
            return line!;
        }
        finally
        {
            await server.StopAsync();
            server.Dispose();
        }
    }

    public static async Task SendAndLeaveAsync(string pipeName, string requestLine)
    {
        using var client = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
        await client.ConnectAsync(TimeSpan.FromSeconds(5), CancellationToken.None);
        await client.WriteAsync(Encoding.UTF8.GetBytes(requestLine + "\n"));
        await client.FlushAsync();
    }
}
