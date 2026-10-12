using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class LocalIpcServerLogicTests
{
    private const string Url = "https://example.com/v";
    private const string CachedMediaUrl = "https://cache.example/v.mp4";
    private const string MeshMediaUrl = "https://media.example/v.mp4";

    [Theory]
    [InlineData(null, "resolved", null, 26_500, true)]
    [InlineData(null, "fallback_native", "all_configs_failed", 26_500, true)]
    [InlineData(null, "fallback_native", "rate_limited", 26_500, true)]
    [InlineData(null, "fallback_native", "protocol_error", 26_500, true)]
    [InlineData(null, "fallback_native", "warp_down", 26_500, true)]
    [InlineData(null, "fallback_native", "server_unreachable", 26_500, false)]
    [InlineData(null, "fallback_native", "internal_error", 26_500, false)]
    [InlineData("server_unreachable", "fallback_native/server_unreachable", null, 26_500, false)]
    [InlineData(null, "fallback_native", "server_unreachable", 3_000, false)]
    [InlineData(null, "fallback_native", "client_deadline_exceeded", 26_500, false)]
    [InlineData(null, "fallback_native", "client_deadline_exceeded", 3_000, true)]
    [InlineData("client_deadline_exceeded", "fallback_native/client_deadline_exceeded", null, 26_500, false)]
    [InlineData("client_deadline_exceeded", "fallback_native/client_deadline_exceeded", null, 3_000, true)]
    [InlineData(null, "fallback_native", "client_deadline_exceeded", 20_000, false)]
    [InlineData(null, "fallback_native", "client_deadline_exceeded", 19_999, true)]
    public void IsHealthyOutcome_CountsServerAndInternalFailuresAsUnhealthy(
        string? failReason, string outcome, string? serverReason, int budgetMs, bool expected)
        => Assert.Equal(expected, LocalIpcServer.IsHealthyOutcome(failReason, outcome, serverReason, budgetMs));

    [Theory]
    [InlineData("cf_403", "cf_403")]
    [InlineData("rate_limited", "rate_limited")]
    [InlineData("sign_in_required", "sign_in_required")]
    [InlineData("content_not_found", "content_not_found")]
    [InlineData("unknown", "unknown")]
    [InlineData("something_else", "unknown")]
    [InlineData(null, "unknown")]
    public void OgFailedDetailFor_CollapsesToTheServerSet(string? wrapperReason, string expected)
        => Assert.Equal(expected, LocalIpcServer.OgFailedDetailFor(wrapperReason));

    [Fact]
    public async Task Probe_request_skips_the_cache()
    {
        var now = new DateTime(2026, 10, 11, 12, 0, 0, DateTimeKind.Utc);
        var gate = OpenGate(() => now);
        now += ResolverHealthGate.DefaultCooldown + TimeSpan.FromSeconds(1);

        await WithSeededCacheAsync(async cache =>
        {
            await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, (connection, ws, id) =>
                ScriptedMeshServer.SendAsync(ws, "{\"action\":\"fallback_native\",\"id\":\"" + id
                    + "\",\"reason\":\"all_configs_failed\"}"));
            await using var client = await ConnectedClientAsync(server);

            string line = await IpcPipeHarness.RoundTripAsync(
                new LocalIpcServer(client, cache, null, gate), NewPipeName(), RequestLine(), TimeSpan.FromSeconds(15));

            using var doc = JsonDocument.Parse(line);
            Assert.Equal(WireConstants.ActionFallbackNative, doc.RootElement.GetProperty("action").GetString());
            Assert.Single(server.Resolves);
            await MeshTestClient.WaitUntilAsync(() => !gate.AwaitingProbeVerdict, 5_000, "probe verdict");
            Assert.False(gate.ShouldShortCircuit(true, out _));
        });
    }

    [Fact]
    public async Task Resolved_probe_comes_from_the_mesh_and_waits_for_playback()
    {
        var now = new DateTime(2026, 10, 11, 12, 0, 0, DateTimeKind.Utc);
        var gate = OpenGate(() => now);
        now += ResolverHealthGate.DefaultCooldown + TimeSpan.FromSeconds(1);

        await WithSeededCacheAsync(async cache =>
        {
            await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept);
            await using var client = await ConnectedClientAsync(server);

            string line = await IpcPipeHarness.RoundTripAsync(
                new LocalIpcServer(client, cache, null, gate), NewPipeName(), RequestLine(), TimeSpan.FromSeconds(15));

            using var doc = JsonDocument.Parse(line);
            Assert.Equal(MeshMediaUrl, doc.RootElement.GetProperty("url").GetString());
            Assert.Single(server.Resolves);
            Assert.True(gate.AwaitingProbeVerdict);
            Assert.Equal(ResolverHealthGate.Transition.Closed, gate.RecordPlaybackConfirmed());
        });
    }

    [Fact]
    public async Task Closed_gate_still_serves_from_the_cache()
    {
        var gate = new ResolverHealthGate();

        await WithSeededCacheAsync(async cache =>
        {
            await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept);
            await using var client = await ConnectedClientAsync(server);

            string line = await IpcPipeHarness.RoundTripAsync(
                new LocalIpcServer(client, cache, null, gate), NewPipeName(), RequestLine(), TimeSpan.FromSeconds(15));

            using var doc = JsonDocument.Parse(line);
            Assert.Equal(CachedMediaUrl, doc.RootElement.GetProperty("url").GetString());
            Assert.Empty(server.Resolves);
        });
    }

    [Theory]
    [InlineData(1_500, false, false)]
    [InlineData(26_500, true, true)]
    [InlineData(1_500, true, false)]
    public async Task Deadline_outcomes_open_the_gate_only_when_the_budget_was_a_full_window(
        int wrapperDeadlineMs, bool serverAnswers, bool expectOpen)
    {
        var gate = new ResolverHealthGate();
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, (connection, ws, id) => serverAnswers
            ? ScriptedMeshServer.SendAsync(ws, "{\"action\":\"fallback_native\",\"id\":\"" + id
                + "\",\"reason\":\"" + WireConstants.FallbackClientDeadlineExceeded + "\",\"public_message\":\"scripted\"}")
            : Task.CompletedTask);
        await using var client = await ConnectedClientAsync(server);
        var ipc = new LocalIpcServer(client, null, null, gate);
        string pipeName = NewPipeName();
        ipc.StartForTests(pipeName);
        try
        {
            var replies = await Task.WhenAll(Enumerable.Range(0, ResolverHealthGate.OpenThreshold)
                .Select(i => AskAsync(pipeName, "{\"action\":\"resolve\",\"id\":\"deadline" + i + "\",\"url\":\"" + Url
                    + "\",\"player\":\"unity\",\"maxHeight\":720,\"wrapper_deadline_ms\":" + wrapperDeadlineMs + "}")));

            foreach (string reply in replies)
            {
                using var doc = JsonDocument.Parse(reply);
                Assert.Equal(WireConstants.FallbackClientDeadlineExceeded, doc.RootElement.GetProperty("reason").GetString());
                Assert.Equal(serverAnswers, doc.RootElement.TryGetProperty("public_message", out _));
            }

            Assert.Equal(expectOpen, gate.ShouldShortCircuit(true, out _));
        }
        finally
        {
            await ipc.StopAsync();
            ipc.Dispose();
        }
    }

    private static async Task<string> AskAsync(string pipeName, string requestLine)
    {
        using var client = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
        await client.ConnectAsync(TimeSpan.FromSeconds(5), CancellationToken.None);
        await client.WriteAsync(Encoding.UTF8.GetBytes(requestLine + "\n"));
        using var reader = new StreamReader(client, Encoding.UTF8, leaveOpen: true);
        string? line = await reader.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(40));
        Assert.False(string.IsNullOrEmpty(line));
        await WaitForHangUpAsync(reader);
        return line!;
    }

    private static async Task WaitForHangUpAsync(StreamReader reader)
    {
        try
        {
            Assert.Null(await reader.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(10)));
        }
        catch (IOException)
        {
        }
    }

    private static ResolverHealthGate OpenGate(Func<DateTime> now)
    {
        var gate = new ResolverHealthGate(
            ResolverHealthGate.DefaultCooldown, ResolverHealthGate.DefaultVerdictTimeout, now);
        for (int i = 0; i < ResolverHealthGate.OpenThreshold; i++)
            gate.RecordResolveOutcome(healthy: false, resolved: false);
        return gate;
    }

    private static string NewPipeName() => "vrcresolver.test." + Guid.NewGuid().ToString("N");

    private static string RequestLine()
        => "{\"action\":\"resolve\",\"id\":\"probe1\",\"url\":\"" + Url + "\",\"player\":\"unity\",\"maxHeight\":720}";

    private static async Task<MeshClient> ConnectedClientAsync(ScriptedMeshServer server)
    {
        var client = MeshTestClient.For(server);
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 5_000, "mesh connected");
        return client;
    }

    private static async Task WithSeededCacheAsync(Func<ResolveCache, Task> body)
    {
        string dir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-probe-" + Guid.NewGuid().ToString("N")[..8]);
        Directory.CreateDirectory(dir);
        var cache = new ResolveCache(Path.Combine(dir, "resolve_cache.json"));
        cache.Store(ServerEndpoints.ProxyHost, Url, WireConstants.PlayerUnity, null, 720,
            new ResolveResponse { Action = WireConstants.ActionResolved, Url = CachedMediaUrl });
        try
        {
            await body(cache);
        }
        finally
        {
            cache.FlushNow();
            try { Directory.Delete(dir, recursive: true); } catch { }
        }
    }
}
