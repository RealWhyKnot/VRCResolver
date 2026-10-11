using System;
using System.Collections.Concurrent;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class MeshFirstConnectRecoveryTests
{
    private static ResolveRequest Request() => new() { Url = "https://www.youtube.com/watch?v=abc", Player = "avpro" };

    [Fact]
    public async Task Client_reconnects_to_the_mesh_after_the_first_connect_is_refused()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Refuse : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server);

        await client.StartAsync();

        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 10_000, "mesh connection after a refused first connect");
        Assert.Equal(2, server.Attempts);
        Assert.Equal(1, server.Connections);
    }

    [Fact]
    public async Task Client_reconnects_to_the_mesh_after_the_first_connect_times_out()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Hang : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server, connectDeadline: TimeSpan.FromMilliseconds(500));

        await client.StartAsync();

        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 10_000, "mesh connection after a timed-out first connect");
        Assert.Equal(2, server.Attempts);
    }

    [Fact]
    public async Task Client_keeps_retrying_the_mesh_until_it_comes_back()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt <= 3 ? MeshAttempt.Refuse : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server);

        await client.StartAsync();

        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 20_000, "mesh connection after three refused connects");
        Assert.Equal(4, server.Attempts);
    }

    [Fact]
    public async Task Every_connect_attempt_after_a_failed_first_connect_targets_the_public_mesh_host()
    {
        var hosts = new ConcurrentQueue<string>();
        await using var server = new ScriptedMeshServer(attempt => attempt <= 2 ? MeshAttempt.Refuse : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server, hosts);

        await client.StartAsync();

        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 15_000, "mesh connection");
        Assert.Equal(3, hosts.Count);
        Assert.All(hosts, h => Assert.Equal(ServerEndpoints.ProxyHost, h));
        Assert.All(server.Paths, p => Assert.Equal("/mesh", p));
    }

    [Fact]
    public async Task Connect_attempts_identify_as_the_current_watchdog()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Refuse : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server);

        await client.StartAsync();

        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 10_000, "mesh connection");
        Assert.All(server.UserAgents, ua => Assert.StartsWith("VRCResolver-Watchdog/", ua));
    }

    [Fact]
    public async Task Resolve_asked_while_the_first_connect_fails_is_served_after_the_retry()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Refuse : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(server, reconnectGrace: TimeSpan.FromSeconds(10));

        await client.StartAsync();
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        var result = await client.ResolveAsync(Request(), cts.Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.Single(server.Resolves);
    }

    [Fact]
    public async Task Client_recovers_after_a_timed_out_first_connect_and_a_refusal()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt switch
        {
            1 => MeshAttempt.Hang,
            2 => MeshAttempt.Refuse,
            _ => MeshAttempt.Accept,
        });
        await using var client = MeshTestClient.For(server, connectDeadline: TimeSpan.FromMilliseconds(500));

        await client.StartAsync();

        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 15_000, "mesh connection after a hang and a refusal");
        Assert.Equal(3, server.Attempts);
        Assert.Equal(1, server.Connections);
    }
}
