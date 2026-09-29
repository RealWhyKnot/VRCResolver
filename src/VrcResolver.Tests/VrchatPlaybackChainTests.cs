using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public sealed class VrchatPlaybackChainTests : IDisposable
{
    private const string ServerManifest = "https://vrcresolver.com/api/proxy/index.m3u8?mode=manifest";
    private static readonly TimeSpan StallWindow = TimeSpan.FromSeconds(2);
    private static readonly TimeSpan Prompt = TimeSpan.FromSeconds(6);

    private readonly string _stateRoot = Path.Combine(Path.GetTempPath(), "VrcResolver.Tests", Guid.NewGuid().ToString("N"));
    private readonly RelayPortManager _ports;
    private readonly ScriptedUpstream _server = new();
    private readonly LocalRelayServer _relay;
    private readonly RelayBypass _bypass;
    private readonly VrcLogMonitor _monitor;
    private readonly WinInetClient _avpro;

    public VrchatPlaybackChainTests()
    {
        WinInetClient.SetMaxConnectionsPerServer(2);
        _ports = new RelayPortManager(_stateRoot);
        Assert.True(_ports.Initialize());
        _ports.WriteSchemeFile("http");
        _relay = new LocalRelayServer(_ports.CurrentPort, "http", _server, TimeSpan.FromSeconds(1));
        _relay.Start();
        _bypass = new RelayBypass(_ports, "http");
        _monitor = new VrcLogMonitor(
            new MeshClient(),
            onRelayWedged: _bypass.Engage,
            onNewVrchatSession: _bypass.Release,
            silentStallWindow: StallWindow);
        _avpro = new WinInetClient();
    }

    public void Dispose()
    {
        _avpro.Dispose();
        _monitor.Dispose();
        _relay.StopAsync().GetAwaiter().GetResult();
        try { Directory.Delete(_stateRoot, recursive: true); } catch { }
    }

    [Fact]
    public async Task ResolvedUrl_PlaysThroughTheRelayAndReachesTheServer()
    {
        string vrchatUrl = TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest);
        Assert.StartsWith("http://localhost.youtube.com:" + _ports.CurrentPort + "/play/", vrchatUrl);

        _monitor.ProcessNewContent("[AVProVideo] Opening " + vrchatUrl + "\n");
        long bytes = await PlayHls(vrchatUrl);

        Assert.Equal(ScriptedUpstream.SegmentCount * (long)ScriptedUpstream.BodyLength, bytes);
        Assert.Contains(_server.Requests, u => u.StartsWith("https://vrcresolver.com/api/proxy/index.m3u8", StringComparison.Ordinal));
        Assert.Equal(ScriptedUpstream.SegmentCount + 1, _server.Requests.Count);
    }

    [Fact]
    public async Task StallAfterTheRelayServedThePlay_IsNotTreatedAsAWedge()
    {
        string vrchatUrl = TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest);
        _monitor.ProcessNewContent("[AVProVideo] Opening " + vrchatUrl + "\n");
        await PlayHls(vrchatUrl);

        await Task.Delay(StallWindow + TimeSpan.FromSeconds(1));

        Assert.False(_bypass.Active);
        Assert.Equal(vrchatUrl.Split("/play/")[0], TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest).Split("/play/")[0]);
    }

    [Fact]
    public async Task WedgedPlayerPool_SwitchesToDirectLinksThenBackOnNextVrchatSession()
    {
        using var stuckA = await Within(() => _avpro.Send(Local(Relay("endless"))));
        using var stuckB = await Within(() => _avpro.Send(Local(Relay("endless"))));
        await Within(() => stuckA.ReadAtLeast(16 * 1024));
        await Within(() => stuckB.ReadAtLeast(16 * 1024));

        string vrchatUrl = TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest);
        _monitor.ProcessNewContent("[AVProVideo] Opening " + vrchatUrl + "\n");
        Task<WinInetClient.Request> queued = Blocking(() => _avpro.Send(Local(vrchatUrl)));

        var sw = Stopwatch.StartNew();
        while (!_bypass.Active)
        {
            Assert.True(sw.Elapsed < StallWindow + Prompt, "watchdog never noticed the jammed relay");
            await Task.Delay(50);
        }
        Assert.False(queued.IsCompleted);
        Assert.Equal(ServerManifest, TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest));

        stuckA.Dispose();
        stuckB.Dispose();
        (await queued.WaitAsync(Prompt)).Dispose();

        _bypass.Release();
        string afterRestart = TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest);
        Assert.StartsWith("http://localhost.youtube.com:" + _ports.CurrentPort + "/play/", afterRestart);
        Assert.Equal(ScriptedUpstream.SegmentCount * (long)ScriptedUpstream.BodyLength, await PlayHls(afterRestart));
    }

    [Fact]
    public void WrapForRelay_FallsBackToTheDirectLinkWheneverTheRelayIsUnusable()
    {
        Assert.Equal("", TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ""));
        Assert.Equal("https://example.com/a.mp4", TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, "https://example.com/a.mp4"));
        Assert.StartsWith("http://localhost.youtube.com:", TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest, probeRelay: true));

        _ports.WriteSchemeFile("https");
        Assert.StartsWith("https://localhost.youtube.com:", TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest));
        File.WriteAllText(Path.Combine(_stateRoot, "relay_scheme.txt"), "ftp");
        Assert.StartsWith("http://localhost.youtube.com:", TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest));

        string portFile = Path.Combine(_stateRoot, "relay_port.txt");
        foreach (string bad in new[] { "", "80", "70000", "abc" })
        {
            File.WriteAllText(portFile, bad);
            Assert.Equal(ServerManifest, TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest));
        }

        File.WriteAllText(portFile, WinInetRelayTests.FreePort().ToString(System.Globalization.CultureInfo.InvariantCulture));
        Assert.Equal(ServerManifest, TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest, probeRelay: true));

        File.Delete(portFile);
        Assert.Equal(ServerManifest, TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, ServerManifest));
    }

    private async Task<long> PlayHls(string vrchatUrl)
    {
        string manifestUrl = Local(vrchatUrl);
        string manifest;
        using (var req = await Within(() => _avpro.Send(manifestUrl)))
        {
            Assert.Equal(200, req.Status);
            manifest = await Within(req.ReadText);
        }

        long total = 0;
        foreach (string line in manifest.Split('\n').Select(l => l.Trim()).Where(l => l.Length > 0 && !l.StartsWith('#')))
        {
            using var seg = await Within(() => _avpro.Send(new Uri(new Uri(manifestUrl), line).ToString()));
            Assert.Equal(200, seg.Status);
            total += await Within(seg.Drain);
        }
        return total;
    }

    private string Relay(string mode)
        => TrustGatewayUrlBuilder.WrapForRelay(_stateRoot, "https://vrcresolver.com/api/proxy/seg.ts?mode=" + mode);

    private static string Local(string vrchatUrl) => vrchatUrl.Replace("//localhost.youtube.com:", "//127.0.0.1:", StringComparison.Ordinal);

    private static Task<T> Blocking<T>(Func<T> work)
        => Task.Factory.StartNew(work, CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);

    private static async Task<T> Within<T>(Func<T> work) => await Blocking(work).WaitAsync(Prompt);
}
