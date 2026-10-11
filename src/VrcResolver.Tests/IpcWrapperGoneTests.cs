using System;
using System.IO;
using System.Net.WebSockets;
using System.Runtime.Versioning;
using System.Threading;
using System.Threading.Tasks;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class IpcWrapperGoneTests : IDisposable
{
    private const string Url = "https://example.com/v";
    private const int MaxHeight = 720;

    private readonly string _tempDir;
    private readonly ResolveCache _cache;
    private int _replies;

    public IpcWrapperGoneTests()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-wrappergone-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_tempDir);
        _cache = new ResolveCache(Path.Combine(_tempDir, "resolve_cache.json"));
    }

    public void Dispose()
    {
        _cache.FlushNow();
        try { Directory.Delete(_tempDir, recursive: true); } catch { }
    }

    private static string NewPipeName() => "vrcresolver.test." + Guid.NewGuid().ToString("N");

    private static string RequestLine(string id)
        => "{\"action\":\"resolve\",\"id\":\"" + id + "\",\"url\":\"" + Url + "\",\"player\":\"unity\",\"maxHeight\":" + MaxHeight + "}";

    private async Task SlowReplyAsync(int connection, WebSocket ws, string id)
    {
        await Task.Delay(300);
        await ScriptedMeshServer.ResolvedAsync(ws, id);
        Interlocked.Increment(ref _replies);
    }

    private static async Task<MeshClient> ConnectedClientAsync(ScriptedMeshServer server)
    {
        var client = MeshTestClient.For(server);
        await client.StartAsync();
        await MeshTestClient.WaitUntilAsync(() => client.IsConnected, 5_000, "mesh connected");
        return client;
    }

    [Fact]
    public async Task Wrapper_leaving_before_the_reply_keeps_the_gate_closed()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, SlowReplyAsync);
        await using var client = await ConnectedClientAsync(server);
        var gate = new ResolverHealthGate();
        var ipc = new LocalIpcServer(client, null, null, gate);
        string pipeName = NewPipeName();
        ipc.StartForTests(pipeName);
        try
        {
            for (int i = 0; i < ResolverHealthGate.OpenThreshold; i++)
                await IpcPipeHarness.SendAndLeaveAsync(pipeName, RequestLine("gone" + i));

            await MeshTestClient.WaitUntilAsync(
                () => Volatile.Read(ref _replies) == ResolverHealthGate.OpenThreshold, 10_000, "mesh replies sent");
            await Task.Delay(1_000);

            Assert.False(gate.ShouldShortCircuit(true, out _));
        }
        finally
        {
            await ipc.StopAsync();
            ipc.Dispose();
        }
    }

    [Fact]
    public async Task Wrapper_leaving_before_the_reply_still_caches_the_result()
    {
        await using var server = new ScriptedMeshServer(_ => MeshAttempt.Accept, SlowReplyAsync);
        await using var client = await ConnectedClientAsync(server);
        var ipc = new LocalIpcServer(client, _cache);
        string pipeName = NewPipeName();
        ipc.StartForTests(pipeName);
        try
        {
            await IpcPipeHarness.SendAndLeaveAsync(pipeName, RequestLine("gone"));

            CachedResolve? cached = null;
            var deadline = DateTime.UtcNow.AddSeconds(8);
            while (cached == null && DateTime.UtcNow < deadline)
            {
                cached = _cache.Lookup(ServerEndpoints.ProxyHost, Url, WireConstants.PlayerUnity, null, MaxHeight, "t");
                if (cached == null) await Task.Delay(50);
            }

            Assert.NotNull(cached);
        }
        finally
        {
            await ipc.StopAsync();
            ipc.Dispose();
        }
    }
}
