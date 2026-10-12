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
    [InlineData(false, 0, true)]
    [InlineData(false, 100, true)]
    [InlineData(true, 1000, false)]
    [InlineData(true, 4999, false)]
    [InlineData(true, 5000, true)]
    [InlineData(true, -1, true)]
    public void ShouldRescanDirectory_follows_current_file_and_cadence(bool hasCurrent, long msSinceScan, bool expected)
    {
        Assert.Equal(expected, VrcLogMonitor.ShouldRescanDirectory(hasCurrent, msSinceScan));
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
    public void ProcessNewContent_waits_for_the_rest_of_a_partial_line()
    {
        using var monitor = new VrcLogMonitor(new MeshClient(), silentStallWindow: TimeSpan.FromMinutes(5));
        const string first = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=first";
        const string second = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=second";

        monitor.ProcessNewContent(
            OpeningLine(first) + "\n"
            + "2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani");

        Assert.Equal(first, monitor.LastOpenedUrlForTests);

        monitor.ProcessNewContent("fest.m3u8?q=second (offset 0) with API MediaFoundation\r\n");

        Assert.Equal(second, monitor.LastOpenedUrlForTests);
    }

    [Fact]
    public void PollOnce_waits_for_a_line_the_game_is_still_writing()
    {
        using var logs = new LogDir();
        using var monitor = new VrcLogMonitor(new MeshClient(), silentStallWindow: TimeSpan.FromMinutes(5),
            vrcDirOverride: logs.Path);
        long ticks = 0;
        const string url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a";

        string log = logs.Write("output_log_a.txt", "2026.10.11 10:00:00 Debug      -  boot\n");
        monitor.PollOnce(ticks += 10_000);

        logs.Append(log, "2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani");
        monitor.PollOnce(ticks += 10_000);
        Assert.Null(monitor.LastOpenedUrlForTests);

        logs.Append(log, "fest.m3u8?q=a (offset 0) with API MediaFoundation\n");
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(url, monitor.LastOpenedUrlForTests);
    }

    [Fact]
    public void Alternating_logs_do_not_replay_old_lines()
    {
        using var logs = new LogDir();
        int sessions = 0;
        using var monitor = new VrcLogMonitor(new MeshClient(), onNewVrchatSession: () => sessions++,
            silentStallWindow: TimeSpan.FromMinutes(5), vrcDirOverride: logs.Path);
        long ticks = 0;
        const string a1 = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a1";
        const string a2 = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a2";
        const string b1 = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b1";

        string a = logs.Write("output_log_a.txt", "2026.10.11 10:00:00 Debug      -  boot\n");
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(0, sessions);

        logs.Append(a, OpeningLine(a1) + "\n");
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(a1, monitor.LastOpenedUrlForTests);
        Assert.Equal(0, sessions);

        string b = logs.Write("output_log_b.txt", OpeningLine(b1) + "\n");
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(b1, monitor.LastOpenedUrlForTests);
        Assert.Equal(1, sessions);

        logs.Touch(a);
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(b1, monitor.LastOpenedUrlForTests);
        Assert.Equal(1, sessions);

        logs.Touch(b);
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(b1, monitor.LastOpenedUrlForTests);
        Assert.Equal(1, sessions);

        logs.Append(a, OpeningLine(a2) + "\n");
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(a2, monitor.LastOpenedUrlForTests);
        Assert.Equal(1, sessions);

        logs.Touch(b);
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(a2, monitor.LastOpenedUrlForTests);
        Assert.Equal(1, sessions);
    }

    [Fact]
    public void A_half_written_line_stays_with_its_own_log_when_the_newest_file_changes()
    {
        using var logs = new LogDir();
        using var monitor = new VrcLogMonitor(new MeshClient(), silentStallWindow: TimeSpan.FromMinutes(5),
            vrcDirOverride: logs.Path);
        long ticks = 0;
        const string a1 = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a1";
        const string b1 = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b1";

        string a = logs.Write("output_log_a.txt", "2026.10.11 10:00:00 Debug      -  boot\n");
        monitor.PollOnce(ticks += 10_000);

        logs.Append(a, "2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani");
        monitor.PollOnce(ticks += 10_000);

        logs.Write("output_log_b.txt", OpeningLine(b1) + "\n");
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(b1, monitor.LastOpenedUrlForTests);

        logs.Append(a, "fest.m3u8?q=a1 (offset 0) with API MediaFoundation\n");
        monitor.PollOnce(ticks += 10_000);
        Assert.Equal(a1, monitor.LastOpenedUrlForTests);
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
        using var monitor = new VrcLogMonitor(new MeshClient(),
            checkAudioOutput: () => new AudioOutputCheck(AudioOutputState.Ok, "Speakers"));

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

    [Theory]
    [InlineData(true, (int)AudioOutputState.InUse, false)]
    [InlineData(true, (int)AudioOutputState.NoDevice, false)]
    [InlineData(true, (int)AudioOutputState.Ok, true)]
    [InlineData(true, (int)AudioOutputState.Unknown, true)]
    [InlineData(false, (int)AudioOutputState.InUse, true)]
    public void Load_failures_are_only_blamed_on_the_resolver_when_sound_output_works(
        bool vrchatReportsNoDevices, int outputState, bool expectBlamed)
    {
        using var temp = new TempDir();
        var cache = temp.NewCache();
        var hint = new OgFallbackHint();
        var gate = new ResolverHealthGate();
        int probes = 0;
        using var monitor = new VrcLogMonitor(new MeshClient(), cache, hint, gate,
            checkAudioOutput: () =>
            {
                probes++;
                return new AudioOutputCheck((AudioOutputState)outputState, "Speakers (3- fifine Microphone)");
            });
        const string sourceUrl = "https://virtualfilm.institute/watch?v=abc";
        const string playbackUrl = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

        if (vrchatReportsNoDevices)
            monitor.ProcessNewContent(
                "2026.10.03 11:55:55 Debug      -  uSpeak: SetInputDevice 0 (0 total, index out of range, setting to default device) 'No device available'\n");
        for (int i = 0; i < ResolverHealthGate.OpenThreshold; i++)
        {
            cache.Store("us1.vrcresolver.com", sourceUrl, "avpro", null, 1080, MakeResolved(playbackUrl));
            monitor.ProcessNewContent(
                "[AVProVideo] Opening " + playbackUrl + "\n"
                + "[AVProVideo] Error: Loading failed\n");
        }

        Assert.Equal(expectBlamed, gate.ShouldShortCircuit(meshConnected: false, out _));
        Assert.Equal(expectBlamed, hint.ShouldPreferOg(sourceUrl));
        Assert.Equal(!expectBlamed, cache.TryGetSourceUrlForResolved(playbackUrl, out _));
        Assert.Equal(vrchatReportsNoDevices ? 1 + ResolverHealthGate.OpenThreshold : 0, probes);
    }

    [Fact]
    public void No_sound_reason_names_the_blocked_device()
    {
        string busy = VrcLogMonitor.NoSoundReason(
            new AudioOutputCheck(AudioOutputState.InUse, "Speakers (3- fifine Microphone)"));
        Assert.Equal("Windows' default playback device 'Speakers (3- fifine Microphone)' is held by another app"
            + " in exclusive mode (Voicemeeter, ASIO or KS outputs can do this)", busy);
        Assert.StartsWith("Windows' default playback device is held",
            VrcLogMonitor.NoSoundReason(new AudioOutputCheck(AudioOutputState.InUse, null)));
        Assert.Equal("Windows has no default playback device",
            VrcLogMonitor.NoSoundReason(new AudioOutputCheck(AudioOutputState.NoDevice, null)));
    }

    [Theory]
    [InlineData(0, "Ok")]
    [InlineData(4, "InUse")]
    [InlineData(2, "NoDevice")]
    [InlineData(6, "NoDevice")]
    [InlineData(1, "Unknown")]
    public void Wave_out_open_results_map_to_output_states(int openResult, string expected)
    {
        Assert.Equal(expected, AudioOutputProbe.Classify(openResult).ToString());
    }

    private static string OpeningLine(string url) =>
        "2026.10.11 10:00:00 Debug      -  [AVProVideo] Opening " + url + " (offset 0) with API MediaFoundation";

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

    private sealed class LogDir : IDisposable
    {
        private DateTime _clock = new(2026, 10, 11, 12, 0, 0, DateTimeKind.Utc);

        public LogDir()
        {
            Path = System.IO.Path.Combine(
                System.IO.Path.GetTempPath(), "vrcresolver-tests-vrcdir-" + Guid.NewGuid().ToString("N")[..8]);
            Directory.CreateDirectory(Path);
        }

        public string Path { get; }

        public string Write(string name, string content)
        {
            string file = System.IO.Path.Combine(Path, name);
            File.WriteAllText(file, content);
            Touch(file);
            return file;
        }

        public void Append(string file, string content)
        {
            File.AppendAllText(file, content);
            Touch(file);
        }

        public void Touch(string file)
        {
            _clock = _clock.AddSeconds(10);
            File.SetLastWriteTimeUtc(file, _clock);
        }

        public void Dispose()
        {
            try { Directory.Delete(Path, recursive: true); } catch { }
        }
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
