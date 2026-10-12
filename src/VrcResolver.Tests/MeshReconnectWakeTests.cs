using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Linq;
using System.Net.WebSockets;
using System.Reflection;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class MeshReconnectWakeTests
{
    private static ResolveRequest Request() => new() { Url = "https://www.youtube.com/watch?v=abc", Player = "avpro" };

    [Fact]
    public async Task Resolve_during_a_long_backoff_wakes_the_reconnect_loop()
    {
        bool allow = false;
        await using var server = new ScriptedMeshServer(_ => Volatile.Read(ref allow) ? MeshAttempt.Accept : MeshAttempt.Refuse);
        await using var client = MeshTestClient.For(server, reconnectGrace: TimeSpan.FromSeconds(10));
        MeshTestClient.Set(client, "_nextBackoffMs", (Func<int, int>)(_ => 30_000));
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => server.Attempts >= 1, 5_000, "first refused attempt");
        Volatile.Write(ref allow, true);

        var sw = Stopwatch.StartNew();
        var result = await client.ResolveAsync(Request(), new CancellationTokenSource(TimeSpan.FromSeconds(20)).Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.True(sw.ElapsedMilliseconds < 3_000, "took " + sw.ElapsedMilliseconds + " ms");
    }

    [Fact]
    public async Task Resolve_waits_for_a_connect_already_in_flight_past_the_grace()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Refuse : MeshAttempt.SlowAccept);
        server.SlowAcceptDelay = TimeSpan.FromSeconds(2);
        await using var client = MeshTestClient.For(
            server,
            connectDeadline: TimeSpan.FromSeconds(5),
            reconnectGrace: TimeSpan.FromSeconds(1));
        MeshTestClient.Set(client, "_nextBackoffMs", (Func<int, int>)(_ => 100));
        await client.StartAsync();

        var result = await client.ResolveAsync(Request(), new CancellationTokenSource(TimeSpan.FromSeconds(20)).Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.Equal(2, server.Attempts);
    }

    [Fact]
    public async Task Resolve_asked_while_a_connect_hangs_is_served_by_the_retry_that_follows_it()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.Hang : MeshAttempt.Accept);
        await using var client = MeshTestClient.For(
            server,
            connectDeadline: TimeSpan.FromMilliseconds(1_200),
            reconnectGrace: TimeSpan.FromSeconds(10));
        MeshTestClient.Set(client, "_nextBackoffMs", (Func<int, int>)(_ => 30_000));
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => server.Attempts >= 1, 5_000, "first hanging attempt");

        var sw = Stopwatch.StartNew();
        var result = await client.ResolveAsync(Request(), new CancellationTokenSource(TimeSpan.FromSeconds(20)).Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.True(sw.ElapsedMilliseconds < 4_000, "took " + sw.ElapsedMilliseconds + " ms");
    }

    [Fact]
    public async Task A_wake_left_over_from_an_earlier_resolve_does_not_skip_the_next_backoff()
    {
        await using var server = new ScriptedMeshServer(attempt => attempt == 1 ? MeshAttempt.SlowAccept : MeshAttempt.Accept);
        server.SlowAcceptDelay = TimeSpan.FromSeconds(1);
        await using var client = MeshTestClient.For(server, reconnectGrace: TimeSpan.FromSeconds(10));
        MeshTestClient.Set(client, "_nextBackoffMs", (Func<int, int>)(_ => 3_000));
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => server.Attempts >= 1, 5_000, "first slow attempt");
        var first = await client.ResolveAsync(Request(), new CancellationTokenSource(TimeSpan.FromSeconds(20)).Token);
        Assert.Equal(WireConstants.ActionResolved, first.Action);

        var ws = (ClientWebSocket)typeof(MeshClient).GetField("_ws", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(client)!;
        ws.Abort();
        await MeshTestClient.WaitUntilAsync(() => !client.IsConnected, 5_000, "dropped link");
        await Task.Delay(1_500);

        Assert.Equal(1, server.Attempts);
        await MeshTestClient.WaitUntilAsync(() => server.Attempts >= 2, 6_000, "reconnect after the backoff");
    }

    [Fact]
    public async Task Resolve_stops_waiting_once_the_in_flight_connect_gives_up()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Hang);
        await using var client = MeshTestClient.For(
            server,
            connectDeadline: TimeSpan.FromMilliseconds(1_500),
            reconnectGrace: TimeSpan.FromSeconds(1));
        MeshTestClient.Set(client, "_nextBackoffMs", (Func<int, int>)(_ => 30_000));
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => server.Attempts >= 1, 5_000, "first hanging attempt");

        var sw = Stopwatch.StartNew();
        var result = await client.ResolveAsync(Request(), new CancellationTokenSource(TimeSpan.FromSeconds(20)).Token);

        Assert.Equal(WireConstants.ActionFallbackNative, result.Action);
        Assert.Equal(WireConstants.FallbackServerUnreachable, result.Reason);
        Assert.InRange(sw.ElapsedMilliseconds, 1_100, 4_000);
    }

    [Fact]
    public async Task A_burst_of_resolves_during_a_backoff_causes_one_extra_attempt_a_second_after_the_last()
    {
        var clock = Stopwatch.StartNew();
        var stamps = new ConcurrentQueue<long>();
        await using var server = new ScriptedMeshServer(_ =>
        {
            stamps.Enqueue(clock.ElapsedMilliseconds);
            return MeshAttempt.Refuse;
        });
        await using var client = MeshTestClient.For(server, reconnectGrace: TimeSpan.FromSeconds(2.5));
        MeshTestClient.Set(client, "_nextBackoffMs", (Func<int, int>)(_ => 30_000));
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => server.Attempts >= 1, 5_000, "first refused attempt");
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(20));

        var results = await Task.WhenAll(Enumerable.Range(0, 10).Select(_ => client.ResolveAsync(Request(), cts.Token)));

        Assert.All(results, r => Assert.Equal(WireConstants.FallbackServerUnreachable, r.Reason));
        var times = stamps.ToArray();
        Assert.Equal(2, times.Length);
        Assert.True(times[1] - times[0] >= 500, "second attempt came " + (times[1] - times[0]) + " ms after the first");
    }

    [Fact]
    public void Default_backoff_keeps_the_per_attempt_caps()
    {
        var field = typeof(MeshClient).GetField("_nextBackoffMs", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.NotNull(field);
        var next = (Func<int, int>)field!.GetValue(new MeshClient())!;
        int[] capsMs = { 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000 };

        for (int attempt = 1; attempt <= capsMs.Length; attempt++)
        {
            int cap = capsMs[attempt - 1];
            var samples = Enumerable.Range(0, 400).Select(_ => next(attempt)).ToArray();
            Assert.All(samples, ms => Assert.InRange(ms, 0, cap));
            Assert.True(samples.Max() > cap / 2, "attempt " + attempt + " never drew above half its cap");
        }
        Assert.InRange(next(500), 0, 30_000);
    }
}
