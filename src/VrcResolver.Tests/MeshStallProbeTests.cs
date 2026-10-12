using System;
using System.Diagnostics;
using System.Linq;
using System.Net.WebSockets;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class MeshStallProbeTests
{
    private static ResolveRequest Request() => new() { Url = "https://www.youtube.com/watch?v=abc", Player = "avpro" };

    private static MeshClient ProbingClient(ScriptedMeshServer server, TimeSpan probeAfter, TimeSpan pongDeadline)
    {
        var client = MeshTestClient.For(server);
        MeshTestClient.Set(client, "_stallProbeAfter", probeAfter);
        MeshTestClient.Set(client, "_stallPongDeadline", pongDeadline);
        return client;
    }

    private static async Task ReplyLaterAsync(WebSocket ws, string id, TimeSpan delay)
    {
        await Task.Delay(delay);
        try { await ScriptedMeshServer.ResolvedAsync(ws, id); }
        catch { }
    }

    [Fact]
    public async Task Resolve_on_a_silent_connection_reconnects_and_resends()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Blackhole : MeshAttempt.Accept);
        await using var client = ProbingClient(server, TimeSpan.FromMilliseconds(300), TimeSpan.FromMilliseconds(300));
        await client.StartAsync();
        var req = Request();

        var sw = Stopwatch.StartNew();
        var result = await client.ResolveAsync(req, new CancellationTokenSource(TimeSpan.FromSeconds(10)).Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.True(sw.ElapsedMilliseconds < 5_000, "took " + sw.ElapsedMilliseconds + " ms");
        Assert.Equal(2, server.Connections);
        var resolves = server.Resolves.ToArray();
        Assert.Contains((1, req.Id), resolves);
        Assert.Contains((2, req.Id), resolves);
    }

    [Fact]
    public async Task Dropping_a_silent_connection_prints_one_console_warning()
    {
        var original = Console.Out;
        var captured = new StringWriter();
        Console.SetOut(captured);
        try
        {
            await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Blackhole : MeshAttempt.Accept);
            await using var client = ProbingClient(server, TimeSpan.FromMilliseconds(300), TimeSpan.FromMilliseconds(300));
            await client.StartAsync();

            var result = await client.ResolveAsync(Request(), new CancellationTokenSource(TimeSpan.FromSeconds(10)).Token);

            Assert.Equal(WireConstants.ActionResolved, result.Action);
        }
        finally
        {
            Console.SetOut(original);
        }

        string output = captured.ToString();
        Assert.Contains("disconnected (error)", output);
        Assert.DoesNotContain("no answer to a ping", output);
    }

    [Fact]
    public async Task Slow_but_alive_server_keeps_its_connection()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, (connection, ws, id) =>
        {
            _ = ReplyLaterAsync(ws, id, TimeSpan.FromMilliseconds(1_500));
            return Task.CompletedTask;
        });
        await using var client = ProbingClient(server, TimeSpan.FromMilliseconds(300), TimeSpan.FromMilliseconds(800));
        await client.StartAsync();

        var result = await client.ResolveAsync(Request(), new CancellationTokenSource(TimeSpan.FromSeconds(10)).Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.Equal(1, server.Connections);
        Assert.NotEmpty(server.Pings);
    }

    [Fact]
    public async Task Two_resolves_on_a_silent_link_are_each_resent_once()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Blackhole : MeshAttempt.Accept);
        await using var client = ProbingClient(server, TimeSpan.FromMilliseconds(300), TimeSpan.FromMilliseconds(300));
        await client.StartAsync();
        var first = Request();
        var second = Request();
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));

        var results = await Task.WhenAll(client.ResolveAsync(first, cts.Token), client.ResolveAsync(second, cts.Token));

        Assert.All(results, r => Assert.Equal(WireConstants.ActionResolved, r.Action));
        var resolves = server.Resolves.ToArray();
        Assert.Contains((1, first.Id), resolves);
        Assert.Contains((1, second.Id), resolves);
        Assert.Equal(1, resolves.Count(r => r == (2, first.Id)));
        Assert.Equal(1, resolves.Count(r => r == (2, second.Id)));
        Assert.Equal(1, server.Pings.Count(c => c == 1));
    }

    [Fact]
    public async Task Cancelling_a_resolve_while_its_probe_waits_returns_at_once()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Blackhole);
        await using var client = ProbingClient(server, TimeSpan.FromMilliseconds(300), TimeSpan.FromSeconds(30));
        await client.StartAsync();
        using var cts = new CancellationTokenSource();
        var resolve = client.ResolveAsync(Request(), cts.Token);
        await MeshTestClient.WaitUntilAsync(() => !server.Pings.IsEmpty, 5_000, "stall probe ping");

        var sw = Stopwatch.StartNew();
        cts.Cancel();
        var result = await resolve;

        Assert.Equal(WireConstants.FallbackClientDeadlineExceeded, result.Reason);
        Assert.True(sw.ElapsedMilliseconds < 1_000, "took " + sw.ElapsedMilliseconds + " ms");
    }
}
