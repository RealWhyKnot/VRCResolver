using System;
using System.Diagnostics;
using System.Linq;
using System.Runtime.Versioning;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class ResolveDeadlineNestingTests
{
    private static string NewPipeName() => "vrcresolver.test." + Guid.NewGuid().ToString("N");

    private static string RequestLine(int? wrapperDeadlineMs)
        => "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"avpro\""
           + (wrapperDeadlineMs is int ms ? ",\"wrapper_deadline_ms\":" + ms : "")
           + "}";

    private static async Task<MeshClient> ConnectedClientAsync(ScriptedMeshServer server)
    {
        var client = MeshTestClient.For(server);
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 5_000, "mesh connected");
        return client;
    }

    private static int SentDeadline(string requestJson)
    {
        using var doc = JsonDocument.Parse(requestJson);
        return doc.RootElement.GetProperty("wrapper_deadline_ms").GetInt32();
    }

    [Theory]
    [InlineData(1_500, 1_000)]
    [InlineData(3_745, 3_245)]
    [InlineData(27_000, 26_500)]
    [InlineData(200_000, 90_000)]
    [InlineData(0, 1_000)]
    public void Effective_budget_never_outlives_the_wrapper(int wrapperMs, int expected)
        => Assert.Equal(expected, LocalIpcServer.EffectiveBudgetMs(wrapperMs));

    [Theory]
    [InlineData(10_000, 9_500)]
    [InlineData(501, 1)]
    [InlineData(500, 0)]
    [InlineData(300, 0)]
    [InlineData(-250, 0)]
    public void Wire_deadline_is_the_time_left_minus_the_margin(int remainingMs, int expected)
    {
        var now = DateTime.UtcNow;
        Assert.Equal(expected, MeshClient.WireDeadlineFor(now.AddMilliseconds(remainingMs), now));
    }

    [Fact]
    public async Task Request_without_a_deadline_keeps_the_callers_wrapper_deadline()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept);
        await using var client = await ConnectedClientAsync(server);
        var req = new ResolveRequest { Url = "https://example.com/v", Player = "avpro", WrapperDeadlineMs = 12_345 };

        var result = await client.ResolveAsync(req, new CancellationTokenSource(TimeSpan.FromSeconds(10)).Token);

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        Assert.Equal(12_345, SentDeadline(Assert.Single(server.RequestJson)));
    }

    [Fact]
    public async Task Resend_after_a_drop_carries_the_time_that_is_left()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, async (connection, ws, id) =>
        {
            if (connection == 1)
            {
                await Task.Delay(400);
                ws.Abort();
                return;
            }
            await ScriptedMeshServer.ResolvedAsync(ws, id);
        });
        await using var client = await ConnectedClientAsync(server);
        var req = new ResolveRequest { Url = "https://example.com/v", Player = "avpro" };
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(20));

        var result = await client.ResolveAsync(req, cts.Token, DateTime.UtcNow.AddSeconds(20));

        Assert.Equal(WireConstants.ActionResolved, result.Action);
        var sent = server.RequestJson.Select(SentDeadline).ToArray();
        Assert.Equal(2, sent.Length);
        Assert.InRange(sent[0], 18_000, 19_500);
        Assert.True(sent[1] <= sent[0] - 300, "sent " + sent[0] + " then " + sent[1]);
    }

    [Fact]
    public async Task Retry_after_discovery_in_progress_sends_a_smaller_wrapper_deadline()
    {
        int answers = 0;
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, (connection, ws, id) =>
            Interlocked.Increment(ref answers) == 1
                ? ScriptedMeshServer.SendAsync(ws, "{\"action\":\"fallback_native\",\"id\":\"" + id
                    + "\",\"reason\":\"discovery_in_progress\",\"retry_after_ms\":800}")
                : ScriptedMeshServer.ResolvedAsync(ws, id));
        await using var client = await ConnectedClientAsync(server);

        string line = await IpcPipeHarness.RoundTripAsync(
            new LocalIpcServer(client), NewPipeName(), RequestLine(20_000), TimeSpan.FromSeconds(15));

        using var doc = JsonDocument.Parse(line);
        Assert.Equal(WireConstants.ActionResolved, doc.RootElement.GetProperty("action").GetString());
        var sent = server.RequestJson.Select(SentDeadline).ToArray();
        Assert.Equal(2, sent.Length);
        Assert.InRange(sent[0], 17_000, 19_000);
        Assert.True(sent[1] <= sent[0] - 700, "sent " + sent[0] + " then " + sent[1]);
    }

    [Fact]
    public async Task Legacy_request_without_a_wrapper_deadline_tells_the_server_less_than_fifteen_seconds()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept);
        await using var client = await ConnectedClientAsync(server);

        string line = await IpcPipeHarness.RoundTripAsync(
            new LocalIpcServer(client), NewPipeName(), RequestLine(null), TimeSpan.FromSeconds(15));

        using var doc = JsonDocument.Parse(line);
        Assert.Equal(WireConstants.ActionResolved, doc.RootElement.GetProperty("action").GetString());
        int sent = SentDeadline(Assert.Single(server.RequestJson));
        Assert.InRange(sent, 10_000, 14_500);
    }

    [Theory]
    [InlineData(1_500)]
    [InlineData(0)]
    public async Task Short_wrapper_budget_is_honoured(int wrapperDeadlineMs)
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, (connection, ws, id) => Task.CompletedTask);
        await using var client = await ConnectedClientAsync(server);

        var sw = Stopwatch.StartNew();
        string line = await IpcPipeHarness.RoundTripAsync(
            new LocalIpcServer(client), NewPipeName(), RequestLine(wrapperDeadlineMs), TimeSpan.FromSeconds(15));
        sw.Stop();

        using var doc = JsonDocument.Parse(line);
        Assert.Equal(WireConstants.ActionFallbackNative, doc.RootElement.GetProperty("action").GetString());
        Assert.Equal(WireConstants.FallbackClientDeadlineExceeded, doc.RootElement.GetProperty("reason").GetString());
        Assert.True(sw.ElapsedMilliseconds < 3_000, "took " + sw.ElapsedMilliseconds + " ms");
    }
}
