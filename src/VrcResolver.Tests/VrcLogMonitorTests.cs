using System.Runtime.Versioning;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public sealed class VrcLogMonitorTests
{
    [Theory]
    [InlineData(true, 1234, 1234)]
    [InlineData(false, 1234, 0)]
    public void InitialReadOffsetForNewFile_TailsOnlyFirstFile(bool firstFile, long fileLength, long expected)
    {
        Assert.Equal(expected, VrcLogMonitor.InitialReadOffsetForNewFile(fileLength, firstFile));
    }

    [Theory]
    [InlineData("[Always] [Video Playback] Switched to 1920x1080", 1920, 1080)]
    [InlineData("[AVProVideo] PostStateChanged: OpeningToPlaying fwidth=1280 fheight=720", 1280, 720)]
    public void TryParseObservedResolution_extracts_width_and_height(string line, int expectedWidth, int expectedHeight)
    {
        Assert.True(VrcLogMonitor.TryParseObservedResolution(line, out int width, out int height));
        Assert.Equal(expectedWidth, width);
        Assert.Equal(expectedHeight, height);
    }

    [Fact]
    public void ProcessNewContent_stores_resolution_for_current_url()
    {
        using var monitor = new VrcLogMonitor(new MeshClient());
        const string url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

        monitor.ProcessNewContent("[AVProVideo] Opening " + url + "\n");
        monitor.ProcessNewContent("[Always] [Video Playback] Switched to 1920x1080\n");

        Assert.Equal(1080, monitor.GetObservedDeliveredHeightForTests(url));
    }

    [Fact]
    public void ProcessNewContent_load_failure_arms_og_hint_before_evicting_cache()
    {
        using var temp = new TempDir();
        var cache = temp.NewCache();
        var hint = new OgFallbackHint();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache, hint);
        const string sourceUrl = "https://virtualfilm.institute/watch?v=abc";
        const string playbackUrl = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

        cache.Store("us1.vrcresolver.com", sourceUrl, "avpro", null, 1080, MakeResolved(playbackUrl));

        monitor.ProcessNewContent(
            "[AVProVideo] Opening " + playbackUrl + "\n"
            + "[AVProVideo] Error: Loading failed\n");

        Assert.True(hint.ShouldPreferOg(sourceUrl));
        Assert.False(cache.TryGetSourceUrlForResolved(playbackUrl, out _));
    }

    [Fact]
    public void MarkPlaybackFailure_arms_og_hint_for_silent_stall_path()
    {
        using var temp = new TempDir();
        var cache = temp.NewCache();
        var hint = new OgFallbackHint();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache, hint);
        const string sourceUrl = "https://virtualfilm.institute/watch?v=abc";
        const string playbackUrl = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

        cache.Store("us1.vrcresolver.com", sourceUrl, "avpro", null, 1080, MakeResolved(playbackUrl));

        var recovery = monitor.MarkPlaybackFailureForTests(playbackUrl);

        Assert.Equal(1, recovery.Evicted);
        Assert.True(recovery.OgHintArmed);
        Assert.True(hint.ShouldPreferOg(sourceUrl));
        Assert.False(cache.TryGetSourceUrlForResolved(playbackUrl, out _));
    }

    [Fact]
    public void Repeated_playback_failures_on_our_urls_open_the_gate()
    {
        var gate = new ResolverHealthGate();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache: null, ogFallbackHint: null, health: gate);

        monitor.MarkPlaybackFailureForTests("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
        monitor.MarkPlaybackFailureForTests("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");
        Assert.False(gate.ShouldShortCircuit(meshConnected: false, out _));
        monitor.MarkPlaybackFailureForTests("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=c");
        Assert.True(gate.ShouldShortCircuit(meshConnected: false, out _));
    }

    [Fact]
    public void Unattributed_playback_failures_do_not_touch_the_gate()
    {
        var gate = new ResolverHealthGate();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache: null, ogFallbackHint: null, health: gate);

        monitor.MarkPlaybackFailureForTests("https://cdn.example.com/video-a.mp4");
        monitor.MarkPlaybackFailureForTests("https://cdn.example.com/video-b.mp4");
        monitor.MarkPlaybackFailureForTests("https://cdn.example.com/video-c.mp4");
        Assert.False(gate.ShouldShortCircuit(meshConnected: false, out _));
    }

    [Fact]
    public void Avpro_accept_line_confirms_playback_and_resets_the_streak()
    {
        var gate = new ResolverHealthGate();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache: null, ogFallbackHint: null, health: gate);
        const string url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

        monitor.MarkPlaybackFailureForTests(url);
        monitor.MarkPlaybackFailureForTests(url);
        monitor.ProcessNewContent(
            "[AVProVideo] Opening " + url + "\n"
            + "[AVProVideo] Using playback path: MediaFoundation\n");
        monitor.MarkPlaybackFailureForTests(url);
        monitor.MarkPlaybackFailureForTests(url);
        Assert.False(gate.ShouldShortCircuit(meshConnected: false, out _));
    }

    [Fact]
    public void Observed_resolution_confirms_playback_for_the_active_url()
    {
        var gate = new ResolverHealthGate();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache: null, ogFallbackHint: null, health: gate);
        const string url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

        monitor.MarkPlaybackFailureForTests(url);
        monitor.MarkPlaybackFailureForTests(url);
        monitor.ProcessNewContent(
            "[AVProVideo] Opening " + url + "\n"
            + "[Always] [Video Playback] Switched to 1920x1080\n");
        monitor.MarkPlaybackFailureForTests(url);
        monitor.MarkPlaybackFailureForTests(url);
        Assert.False(gate.ShouldShortCircuit(meshConnected: false, out _));
    }

    [Fact]
    public async Task Leaving_the_world_mid_load_is_not_a_failed_play()
    {
        var gate = new ResolverHealthGate();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache: null, ogFallbackHint: null, health: gate,
            silentStallWindow: TimeSpan.FromMilliseconds(100));
        monitor.MarkPlaybackFailureForTests("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
        monitor.MarkPlaybackFailureForTests("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");

        monitor.ProcessNewContent(
            "2026.09.29 13:23:56 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c (offset 0) with API MediaFoundation\n"
            + "2026.09.29 13:23:59 Debug      -  [Behaviour] OnLeftRoom\n"
            + "2026.09.29 13:24:00 Error      -  [AVProVideo] Error: Loading failed.  File not found, codec not supported, video resolution too high or insufficient system resources.\n");
        await Task.Delay(400);

        Assert.False(gate.ShouldShortCircuit(meshConnected: false, out _));
    }

    [Fact]
    public async Task A_stalled_play_in_the_same_world_still_counts()
    {
        var gate = new ResolverHealthGate();
        using var monitor = new VrcLogMonitor(new MeshClient(), cache: null, ogFallbackHint: null, health: gate,
            silentStallWindow: TimeSpan.FromMilliseconds(100));
        monitor.MarkPlaybackFailureForTests("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
        monitor.MarkPlaybackFailureForTests("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");

        monitor.ProcessNewContent(
            "2026.09.29 13:24:01 Debug      -  [Behaviour] Entering Room: Hazy Glow\n"
            + "2026.09.29 13:24:16 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c (offset 0) with API MediaFoundation\n");

        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(3);
        while (!gate.ShouldShortCircuit(meshConnected: false, out _) && DateTime.UtcNow < deadline)
            await Task.Delay(25);
        Assert.True(gate.ShouldShortCircuit(meshConnected: false, out _));
    }

    [Fact]
    public void Tracks_when_vrchat_reports_no_audio_devices()
    {
        using var monitor = new VrcLogMonitor(new MeshClient());

        monitor.ProcessNewContent(
            "2026.09.29 13:23:29 Debug      -  uSpeak: SetInputDevice 0 (0 total, index out of range, setting to default device) 'No device available'\n");
        Assert.True(monitor.AudioDevicesMissingForTests);

        monitor.ProcessNewContent(
            "2026.09.29 13:24:09 Debug      -  uSpeak: SetInputDevice 0 (0 total, index out of range, setting to default device) 'No device available'\n");
        Assert.True(monitor.AudioDevicesMissingForTests);

        monitor.ProcessNewContent(
            "2026.09.29 13:26:28 Debug      -  uSpeak: SetInputDevice 0 (5 total) 'Microphone (2- fifine Microphone)'\n");
        Assert.False(monitor.AudioDevicesMissingForTests);
    }

    private static ResolveResponse MakeResolved(string playbackUrl)
    {
        return new ResolveResponse
        {
            Action = WireConstants.ActionResolved,
            Id = "ignored-on-store",
            Url = playbackUrl,
            Engine = "yt-dlp:no-cookies-default",
            Container = "mp4",
            VideoCodec = "h264",
            AudioCodec = "aac",
            Protocol = "https",
            AudioChannels = 2,
            ExpiresAt = DateTime.UtcNow.AddHours(1).ToString("o"),
        };
    }

    private sealed class TempDir : IDisposable
    {
        private readonly string _path;

        public TempDir()
        {
            _path = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-vrclog-" + Guid.NewGuid().ToString("N")[..8]);
            Directory.CreateDirectory(_path);
            ResolveCachePath = Path.Combine(_path, "resolve_cache.json");
        }

        private readonly List<ResolveCache> _caches = new();

        public string ResolveCachePath { get; }

        public ResolveCache NewCache()
        {
            var cache = new ResolveCache(ResolveCachePath);
            _caches.Add(cache);
            return cache;
        }

        public void Dispose()
        {
            foreach (var cache in _caches) cache.FlushNow();
            try { Directory.Delete(_path, recursive: true); } catch { }
        }
    }
}
