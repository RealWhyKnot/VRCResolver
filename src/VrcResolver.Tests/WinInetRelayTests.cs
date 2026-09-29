using System.Collections.Concurrent;
using System.Diagnostics;
using System.Net;
using System.Net.Http.Headers;
using System.Net.Sockets;
using System.Runtime.Versioning;
using System.Text;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public sealed class WinInetRelayTests : IDisposable
{
    private const int VrchatDefaultLimit = 2;
    private const int PlaysPastTheLimit = VrchatDefaultLimit * 2 + 1;
    private static readonly TimeSpan RelayTimeout = TimeSpan.FromSeconds(1);
    private static readonly TimeSpan Prompt = TimeSpan.FromSeconds(6);

    private readonly int _port = FreePort();
    private readonly ScriptedUpstream _upstream = new();
    private readonly LocalRelayServer _relay;
    private readonly WinInetClient _player;

    public WinInetRelayTests()
    {
        WinInetClient.SetMaxConnectionsPerServer(VrchatDefaultLimit);
        _relay = new LocalRelayServer(_port, "http", _upstream, RelayTimeout);
        _relay.Start();
        _player = new WinInetClient();
    }

    public void Dispose()
    {
        _player.Dispose();
        _relay.StopAsync().GetAwaiter().GetResult();
        WinInetClient.SetMaxConnectionsPerServer(VrchatDefaultLimit);
    }

    [Fact]
    public async Task Harness_ReproducesTheWedgeWhenTwoPlayerConnectionsNeverFinish()
    {
        Assert.Equal(VrchatDefaultLimit, WinInetClient.GetMaxConnectionsPerServer());
        using var first = await Within(() => _player.Send(PlayUrl("endless")));
        using var second = await Within(() => _player.Send(PlayUrl("endless")));
        await Within(() => first.ReadAtLeast(64 * 1024));
        await Within(() => second.ReadAtLeast(64 * 1024));

        Task<WinInetClient.Request> third = Blocking(() => _player.Send(PlayUrl("full")));
        await Task.Delay(TimeSpan.FromSeconds(2));
        Assert.False(third.IsCompleted, "third play should queue behind two held connections");

        first.Dispose();
        second.Dispose();
        using var unblocked = await third.WaitAsync(Prompt);
        Assert.Equal(200, unblocked.Status);
        Assert.Equal(ScriptedUpstream.BodyLength, await Within(unblocked.Drain));
    }

    [Theory]
    [InlineData("short")]
    [InlineData("reset")]
    [InlineData("stall-body")]
    [InlineData("stall-headers")]
    [InlineData("status-500")]
    [InlineData("status-404")]
    [InlineData("manifest-stall")]
    public async Task FailedPlays_NeverStrandThePlayerConnection(string mode)
    {
        string ext = mode.StartsWith("manifest", StringComparison.Ordinal) ? "m3u8" : "ts";
        for (int i = 0; i < PlaysPastTheLimit; i++)
        {
            var sw = Stopwatch.StartNew();
            await Within(() =>
            {
                try
                {
                    using var req = _player.Send(PlayUrl(mode, ext));
                    req.Drain();
                }
                catch (System.ComponentModel.Win32Exception) { }
                return 0;
            });
            Assert.True(sw.Elapsed < Prompt, mode + " play " + i + " held the player for " + sw.Elapsed.TotalSeconds + "s");
        }

        await AssertNextPlayGoesThrough();
    }

    [Fact]
    public async Task PlayerSwitchingVideoMidStream_ReleasesBothSidesOfTheRelay()
    {
        for (int i = 0; i < PlaysPastTheLimit * 2; i++)
        {
            using var req = await Within(() => _player.Send(PlayUrl("endless")));
            Assert.Equal(200, req.Status);
            Assert.True(await Within(() => req.ReadAtLeast(128 * 1024)) >= 128 * 1024);
        }

        await AssertNextPlayGoesThrough();
        await WaitUntil(() => _upstream.OpenBodies == 0, "relay kept reading upstream after the player left");
    }

    [Fact]
    public async Task SeekingWithRangeRequests_ReleasesEveryConnection()
    {
        for (int i = 0; i < PlaysPastTheLimit * 2; i++)
        {
            long from = i * 10_000L;
            using var req = await Within(() => _player.Send(PlayUrl("full"), headers: "Range: bytes=" + from + "-\r\n"));
            Assert.Equal(206, req.Status);
            await Within(() => req.ReadAtLeast(8 * 1024));
        }

        await AssertNextPlayGoesThrough();
        Assert.Contains(_upstream.RangeHeaders, r => r == "bytes=10000-");
    }

    [Fact]
    public async Task HeadProbes_DoNotHoldConnections()
    {
        for (int i = 0; i < PlaysPastTheLimit; i++)
        {
            using var req = await Within(() => _player.Send(PlayUrl("full"), verb: "HEAD"));
            Assert.Equal(200, req.Status);
        }

        await AssertNextPlayGoesThrough();
    }

    [Fact]
    public async Task ManyBackToBackPlays_AllFinish()
    {
        for (int i = 0; i < 30; i++)
        {
            using var req = await Within(() => _player.Send(PlayUrl("full")));
            Assert.Equal(200, req.Status);
            Assert.Equal(ScriptedUpstream.BodyLength, await Within(req.Drain));
        }
    }

    [Fact]
    public async Task HlsPlay_FetchesManifestAndEverySegmentThroughTheRelayToTheServer()
    {
        for (int play = 0; play < PlaysPastTheLimit; play++)
        {
            _upstream.Requests.Clear();
            string manifestUrl = PlayUrl("manifest", "m3u8");
            string manifest;
            using (var req = await Within(() => _player.Send(manifestUrl)))
            {
                Assert.Equal(200, req.Status);
                manifest = await Within(req.ReadText);
            }

            string[] segments = manifest.Split('\n')
                .Select(l => l.Trim())
                .Where(l => l.Length > 0 && !l.StartsWith('#'))
                .ToArray();
            Assert.Equal(ScriptedUpstream.SegmentCount, segments.Length);
            Assert.DoesNotContain(segments, s => s.Contains("vrcresolver.com", StringComparison.OrdinalIgnoreCase));

            foreach (string segment in segments)
            {
                string segUrl = new Uri(new Uri(manifestUrl), segment).ToString();
                using var req = await Within(() => _player.Send(segUrl));
                Assert.Equal(200, req.Status);
                Assert.Equal(ScriptedUpstream.BodyLength, await Within(req.Drain));
            }

            string[] seen = _upstream.Requests.ToArray();
            Assert.Contains(seen, u => u.StartsWith("https://vrcresolver.com/api/proxy/index.m3u8", StringComparison.Ordinal));
            for (int i = 0; i < ScriptedUpstream.SegmentCount; i++)
            {
                int n = i;
                Assert.Contains(seen, u => u.StartsWith("https://vrcresolver.com/api/proxy/seg" + n + ".ts", StringComparison.Ordinal));
            }
        }
    }

    [Fact]
    public async Task RelayShutdownMidStream_ReleasesThePlayer()
    {
        var req = await Within(() => _player.Send(PlayUrl("endless")));
        try
        {
            await Within(() => req.ReadAtLeast(64 * 1024));
            Task<long> reading = Blocking(() =>
            {
                try { return req.Drain(); }
                catch (System.ComponentModel.Win32Exception) { return -1; }
            });
            await _relay.StopAsync();
            await reading.WaitAsync(Prompt);
        }
        finally
        {
            req.Dispose();
        }
    }

    [Fact]
    public async Task RaisedConnectionLimit_KeepsNewPlaysFlowingAlongsideLongStreams()
    {
        WinInetClient.SetMaxConnectionsPerServer(WinInetConnectionLimit.Target);
        var held = new List<WinInetClient.Request>();
        try
        {
            for (int i = 0; i < WinInetConnectionLimit.Target - 1; i++)
            {
                var req = await Within(() => _player.Send(PlayUrl("endless")));
                held.Add(req);
                await Within(() => req.ReadAtLeast(16 * 1024));
            }

            await AssertNextPlayGoesThrough();
        }
        finally
        {
            foreach (var req in held) req.Dispose();
        }
    }

    private async Task AssertNextPlayGoesThrough()
    {
        using var req = await Within(() => _player.Send(PlayUrl("full")));
        Assert.Equal(200, req.Status);
        Assert.Equal(ScriptedUpstream.BodyLength, await Within(req.Drain));
    }

    private string PlayUrl(string mode, string ext = "ts")
    {
        string name = mode == "manifest" || mode == "manifest-stall" ? "index" : "seg";
        string target = "https://vrcresolver.com/api/proxy/" + name + "." + ext + "?mode=" + mode;
        return "http://127.0.0.1:" + _port + "/play/" + Guid.NewGuid().ToString("N")[..12] + "/manifest." + ext
            + "?target=" + LocalRelayTargetResolver.EncodeTargetParam(target);
    }

    private static Task<T> Blocking<T>(Func<T> work)
        => Task.Factory.StartNew(work, CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);

    private static async Task<T> Within<T>(Func<T> work)
    {
        try { return await Blocking(work).WaitAsync(Prompt); }
        catch (TimeoutException) { throw new TimeoutException("player call blocked for more than " + Prompt.TotalSeconds + "s"); }
    }

    private static async Task WaitUntil(Func<bool> condition, string failure)
    {
        var sw = Stopwatch.StartNew();
        while (!condition())
        {
            Assert.True(sw.Elapsed < Prompt, failure);
            await Task.Delay(50);
        }
    }

    internal static int FreePort()
    {
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        int port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        return port;
    }
}
