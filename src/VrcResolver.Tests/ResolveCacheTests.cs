using System.Runtime.Versioning;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class ResolveCacheTests : IDisposable
{
    private readonly string _tempDir;
    private readonly string _path;

    public ResolveCacheTests()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-resolvecache-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_tempDir);
        _path = Path.Combine(_tempDir, "resolve_cache.json");
    }

    public void Dispose()
    {
        try { Directory.Delete(_tempDir, recursive: true); } catch { }
    }

    private static ResolveResponse MakeResolved(string url, string? expiresAt)
    {
        return new ResolveResponse
        {
            Action = WireConstants.ActionResolved,
            Id = "ignored-on-store",
            Url = "https://stream.example.com/" + Guid.NewGuid().ToString("N"),
            Engine = "yt-dlp:no-cookies-default",
            Container = "mp4",
            VideoCodec = "h264",
            AudioCodec = "aac",
            Protocol = "https",
            AudioChannels = 2,
            ExpiresAt = expiresAt,
        };
    }

    [Fact]
    public void Lookup_on_missing_file_returns_null()
    {
        var cache = new ResolveCache(_path);
        Assert.Null(cache.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, "req-1"));
    }

    [Fact]
    public void Store_then_Lookup_round_trips_with_id_restamped()
    {
        var cache = new ResolveCache(_path);
        var resp = MakeResolved("https://www.youtube.com/watch?v=x", DateTime.UtcNow.AddHours(2).ToString("o"));

        cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", "(mp4/best)[height<=?1080]", 1080, resp);

        var hit = cache.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", "(mp4/best)[height<=?1080]", 1080, "req-2");
        Assert.NotNull(hit);
        Assert.Equal(WireConstants.ActionResolved, hit.Value.Action);

        string json = System.Text.Encoding.UTF8.GetString(hit.Value.Frame);
        Assert.Contains("\"id\":\"req-2\"", json);
        Assert.DoesNotContain("ignored-on-store", json);

        Assert.Contains(resp.Url!, json);
    }

    [Fact]
    public void Lookup_with_different_player_misses()
    {
        var cache = new ResolveCache(_path);
        var resp = MakeResolved("https://www.youtube.com/watch?v=x", DateTime.UtcNow.AddHours(1).ToString("o"));
        cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", "fmt", 1080, resp);

        Assert.Null(cache.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "unity", "fmt", 1080, "r"));
    }

    [Fact]
    public void Lookup_with_different_format_misses()
    {
        var cache = new ResolveCache(_path);
        var resp = MakeResolved("https://www.youtube.com/watch?v=x", DateTime.UtcNow.AddHours(1).ToString("o"));
        cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", "fmt-A", 1080, resp);

        Assert.Null(cache.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", "fmt-B", 1080, "r"));
    }

    [Fact]
    public void Lookup_with_different_node_misses()
    {
        var cache = new ResolveCache(_path);
        var resp = MakeResolved("https://www.youtube.com/watch?v=x", DateTime.UtcNow.AddHours(1).ToString("o"));
        cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", "fmt", 1080, resp);

        Assert.Null(cache.Lookup("eu1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", "fmt", 1080, "r"));
    }

    [Fact]
    public void Store_skips_fallback_native_responses()
    {
        var cache = new ResolveCache(_path);
        var resp = new ResolveResponse
        {
            Action = WireConstants.ActionFallbackNative,
            Reason = "discovery_in_progress",
            ExpiresAt = DateTime.UtcNow.AddHours(1).ToString("o"),
        };
        cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, resp);

        Assert.Equal(0, cache.Count);
    }

    [Fact]
    public void Store_applies_fallback_default_TTL_when_server_omits_expires_at()
    {
        var cache = new ResolveCache(_path);
        var resp = MakeResolved("https://www.youtube.com/watch?v=x", expiresAt: null);
        string? effective = cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, resp);

        Assert.NotNull(effective);
        Assert.Equal(1, cache.Count);

        var hit = cache.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, "r");
        Assert.NotNull(hit);
    }

    [Fact]
    public void Lookup_treats_expired_entry_as_miss_and_evicts_it()
    {
        var cache = new ResolveCache(_path);
        var resp = MakeResolved("https://www.youtube.com/watch?v=x", DateTime.UtcNow.AddSeconds(-60).ToString("o"));
        resp.ExpiresAt = DateTime.UtcNow.AddSeconds(5).ToString("o");
        cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, resp);
        Assert.Equal(1, cache.Count);

        Assert.Null(cache.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, "r"));

        Assert.Equal(0, cache.Count);
    }

    [Fact]
    public void EvictByUrl_drops_all_entries_for_that_url_across_all_player_format_combos()
    {
        var cache = new ResolveCache(_path);
        string url = "https://www.youtube.com/watch?v=stale";
        string future = DateTime.UtcNow.AddHours(1).ToString("o");

        cache.Store("us1.vrcresolver.com", url, "avpro", "fmt-A", 1080, MakeResolved(url, future));
        cache.Store("us1.vrcresolver.com", url, "avpro", "fmt-B", 1080, MakeResolved(url, future));
        cache.Store("us1.vrcresolver.com", url, "unity", null, 1080, MakeResolved(url, future));
        cache.Store("us1.vrcresolver.com", "https://other.example.com/x", "avpro", null, 1080, MakeResolved("other", future));
        Assert.Equal(4, cache.Count);

        int evicted = cache.EvictByUrl(url);
        Assert.Equal(3, evicted);
        Assert.Equal(1, cache.Count);

        Assert.Null(cache.Lookup("us1.vrcresolver.com", url, "avpro", "fmt-A", 1080, "r"));
        Assert.Null(cache.Lookup("us1.vrcresolver.com", url, "avpro", "fmt-B", 1080, "r"));
        Assert.NotNull(cache.Lookup("us1.vrcresolver.com", "https://other.example.com/x", "avpro", null, 1080, "r"));
    }

    [Fact]
    public void EvictByUrl_drops_entries_by_resolved_playback_url()
    {
        var cache = new ResolveCache(_path);
        string sourceUrl = "https://www.youtube.com/watch?v=stale";
        string playbackUrl = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";
        string future = DateTime.UtcNow.AddHours(1).ToString("o");
        var resp = new ResolveResponse
        {
            Action = WireConstants.ActionResolved,
            Id = "ignored-on-store",
            Url = playbackUrl,
            Engine = "yt-dlp:no-cookies-default",
            Protocol = "hls",
            ExpiresAt = future,
        };

        cache.Store("us1.vrcresolver.com", sourceUrl, "avpro", null, 1080, resp);
        int evicted = cache.EvictByUrl(playbackUrl);

        Assert.Equal(1, evicted);
        Assert.Null(cache.Lookup("us1.vrcresolver.com", sourceUrl, "avpro", null, 1080, "r"));
    }

    [Fact]
    public void TryGetSourceUrlForResolved_RoundTripsAfterStore()
    {
        var cache = new ResolveCache(_path);
        const string sourceUrl = "https://www.youtube.com/watch?v=abc";
        const string playbackUrl = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=xyz";
        var resp = new ResolveResponse
        {
            Action = WireConstants.ActionResolved,
            Id = "ignored-on-store",
            Url = playbackUrl,
            Engine = "yt-dlp:no-cookies-default",
            Protocol = "hls",
            ExpiresAt = DateTime.UtcNow.AddHours(1).ToString("o"),
        };
        cache.Store("us1.vrcresolver.com", sourceUrl, "avpro", null, 1080, resp);

        Assert.True(cache.TryGetSourceUrlForResolved(playbackUrl, out string recovered));
        Assert.Equal(sourceUrl, recovered);
    }

    [Fact]
    public void TryGetSourceUrlForResolved_FalseWhenResolvedUrlUnknown()
    {
        var cache = new ResolveCache(_path);
        Assert.False(cache.TryGetSourceUrlForResolved(
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=missing",
            out string source));
        Assert.Equal("", source);
    }

    [Fact]
    [SupportedOSPlatform("windows")]
    public void VrcLogMonitor_CanonicalizesLocalTrustGatewayUrlToResolvedTarget()
    {
        const string playbackUrl = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";
        Assert.True(TrustGatewayUrlBuilder.TryBuild(
            51234,
            playbackUrl,
            "session",
            out string localUrl));

        Assert.Equal(playbackUrl, VrcLogMonitor.CanonicalPlaybackObservationUrl(localUrl));
        Assert.Equal(playbackUrl, VrcLogMonitor.CanonicalPlaybackObservationUrl(playbackUrl));

        string nonFirstPartyTarget = Base64UrlText.Encode("https://cdn.example.com/video.m3u8");
        string forgedLocal = "http://localhost.youtube.com:51234/play/session/manifest.m3u8?target="
            + nonFirstPartyTarget;
        Assert.Equal(forgedLocal, VrcLogMonitor.CanonicalPlaybackObservationUrl(forgedLocal));
    }

    [Fact]
    public void Cap_evicts_oldest_fetched_at_first()
    {
        var cache = new ResolveCache(_path);
        string future = DateTime.UtcNow.AddHours(1).ToString("o");

        for (int i = 0; i < 502; i++)
        {
            string url = "https://example.com/v=" + i;
            cache.Store("us1.vrcresolver.com", url, "avpro", null, 1080, MakeResolved(url, future));
            System.Threading.Thread.Sleep(1);
        }
        Assert.Equal(500, cache.Count);

        Assert.Null(cache.Lookup("us1.vrcresolver.com", "https://example.com/v=0", "avpro", null, 1080, "r"));
        Assert.Null(cache.Lookup("us1.vrcresolver.com", "https://example.com/v=1", "avpro", null, 1080, "r"));
        Assert.NotNull(cache.Lookup("us1.vrcresolver.com", "https://example.com/v=501", "avpro", null, 1080, "r"));
    }

    [Fact]
    public void Persisted_state_survives_FlushNow_and_a_fresh_instance_load()
    {
        var first = new ResolveCache(_path);
        var resp = MakeResolved("https://www.youtube.com/watch?v=persist", DateTime.UtcNow.AddHours(2).ToString("o"));
        first.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=persist", "avpro", "fmt", 1080, resp);
        first.FlushNow();
        Assert.True(File.Exists(_path));

        var second = new ResolveCache(_path);
        var hit = second.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=persist", "avpro", "fmt", 1080, "r-after-restart");
        Assert.NotNull(hit);
        string json = System.Text.Encoding.UTF8.GetString(hit.Value.Frame);
        Assert.Contains("\"id\":\"r-after-restart\"", json);
    }

    [Fact]
    public void Load_drops_already_expired_entries_so_stale_urls_arent_resurrected()
    {
        string fresh = DateTime.UtcNow.AddHours(2).ToString("o");
        string stale = DateTime.UtcNow.AddSeconds(5).ToString("o");
        string handCrafted = "{\"version\":1,\"entries\":{" +
            "\"keep\":{\"key\":\"keep\",\"node\":\"n\",\"url\":\"u-keep\",\"player\":\"avpro\",\"format_arg\":\"f\",\"response\":{\"action\":\"resolved\",\"id\":\"i\",\"url\":\"https://x/\",\"expires_at\":\"" + fresh + "\"},\"fetched_at\":\"2026-01-01T00:00:00Z\",\"expires_at\":\"" + fresh + "\"}," +
            "\"drop\":{\"key\":\"drop\",\"node\":\"n\",\"url\":\"u-drop\",\"player\":\"avpro\",\"format_arg\":\"f\",\"response\":{\"action\":\"resolved\",\"id\":\"i\",\"url\":\"https://x/\",\"expires_at\":\"" + stale + "\"},\"fetched_at\":\"2026-01-01T00:00:00Z\",\"expires_at\":\"" + stale + "\"}" +
            "}}";
        File.WriteAllText(_path, handCrafted);

        var cache = new ResolveCache(_path);
        Assert.Equal(1, cache.Count);
    }

    [Fact]
    public void Corrupt_file_loads_as_empty_cache()
    {
        File.WriteAllText(_path, "this is not json{[}");
        var cache = new ResolveCache(_path);
        Assert.Equal(0, cache.Count);
        var resp = MakeResolved("https://www.youtube.com/watch?v=x", DateTime.UtcNow.AddHours(1).ToString("o"));
        cache.Store("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, resp);
        Assert.NotNull(cache.Lookup("us1.vrcresolver.com", "https://www.youtube.com/watch?v=x", "avpro", null, 1080, "r"));
    }

    [Fact]
    public void Oversized_file_renames_aside_and_treats_as_miss()
    {
        File.WriteAllBytes(_path, new byte[ResolveCache.MaxCacheFileBytes + 1]);

        var cache = new ResolveCache(_path);
        Assert.Equal(0, cache.Count);

        Assert.False(File.Exists(_path));
        var aside = Directory.GetFiles(_tempDir, "resolve_cache.json.oversized-*");
        Assert.Single(aside);
    }
    [Fact]
    public void DifferentMaxHeightsDoNotShareAnEntry()
    {
        var cache = new ResolveCache(_path);
        const string url = "https://www.youtube.com/watch?v=q";
        var resp = MakeResolved(url, DateTime.UtcNow.AddHours(1).ToString("o"));

        cache.Store("us1.vrcresolver.com", url, "avpro", null, 1080, resp);

        Assert.Null(cache.Lookup("us1.vrcresolver.com", url, "avpro", null, 2160, "r"));
        Assert.NotNull(cache.Lookup("us1.vrcresolver.com", url, "avpro", null, 1080, "r"));
    }

}
