using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class V3WelcomeCacheTests : IDisposable
{
    private readonly string _tempDir;
    private readonly string _path;

    public V3WelcomeCacheTests()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "vrcresolver-tests-v3cache-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(_tempDir);
        _path = Path.Combine(_tempDir, "v3_welcome_cache.json");
    }

    public void Dispose()
    {
        try { Directory.Delete(_tempDir, recursive: true); } catch { }
    }

    [Fact]
    public void Get_on_missing_file_returns_null()
    {
        var cache = new WelcomeCache(_path);
        Assert.Null(cache.Get("us1.vrcresolver.com"));
    }

    [Fact]
    public void Store_then_Get_round_trips_per_node()
    {
        var cache = new WelcomeCache(_path);
        var welcome1 = new WelcomeFrame
        {
            ProtocolVersion = 3,
            Node = "node1",
            Engines = new[] { "yt-dlp" },
            Features = new[] { "v3_compression" },
            WarpActive = true,
            ServerVersion = "v1",
            YtDlpVersion = "yd1",
        };
        var welcome2 = new WelcomeFrame
        {
            ProtocolVersion = 3,
            Node = "node2",
            Engines = new[] { "yt-dlp", "ffmpeg" },
            Features = new[] { "v3_compression", "welcome_hash_ack" },
            WarpActive = false,
            ServerVersion = "v2",
            YtDlpVersion = "yd2",
        };
        cache.Store("us1.vrcresolver.com", welcome1, "hash1");
        cache.Store("eu1.vrcresolver.com", welcome2, "hash2");

        var reopened = new WelcomeCache(_path);
        var got1 = reopened.Get("us1.vrcresolver.com");
        var got2 = reopened.Get("eu1.vrcresolver.com");
        Assert.NotNull(got1);
        Assert.NotNull(got2);
        Assert.Equal("hash1", got1!.WelcomeHash);
        Assert.Equal("node1", got1.Node);
        Assert.Equal(true, got1.WarpActive);
        Assert.Equal("hash2", got2!.WelcomeHash);
        Assert.Equal(2, got2.Engines!.Length);
        Assert.Equal(false, got2.WarpActive);
    }

    [Fact]
    public void Store_for_one_node_does_not_evict_another()
    {
        var cache = new WelcomeCache(_path);
        cache.Store("us1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3, Node = "node1" }, "h1");
        cache.Store("eu1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3, Node = "node2" }, "h2");
        Assert.Equal("h1", cache.Get("us1.vrcresolver.com")?.WelcomeHash);
        Assert.Equal("h2", cache.Get("eu1.vrcresolver.com")?.WelcomeHash);
    }

    [Fact]
    public void Invalidate_removes_only_the_named_node()
    {
        var cache = new WelcomeCache(_path);
        cache.Store("us1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3 }, "h1");
        cache.Store("eu1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3 }, "h2");
        cache.Invalidate("us1.vrcresolver.com");
        Assert.Null(cache.Get("us1.vrcresolver.com"));
        Assert.NotNull(cache.Get("eu1.vrcresolver.com"));
    }

    [Fact]
    public void Get_on_corrupt_file_returns_null()
    {
        File.WriteAllText(_path, "{ this is not valid json");
        var cache = new WelcomeCache(_path);
        Assert.Null(cache.Get("us1.vrcresolver.com"));
    }

    [Fact]
    public void Store_writes_atomically_via_tmp_then_rename()
    {
        var cache = new WelcomeCache(_path);
        cache.Store("node1", new WelcomeFrame { ProtocolVersion = 3 }, "h");
        Assert.True(File.Exists(_path));
        Assert.False(File.Exists(_path + ".new"));
    }

    [Fact]
    public void Get_on_empty_or_null_node_returns_null()
    {
        var cache = new WelcomeCache(_path);
        Assert.Null(cache.Get(""));
        Assert.Null(cache.Get(null!));
    }

    [Fact]
    public void Store_on_empty_hash_is_noop()
    {
        var cache = new WelcomeCache(_path);
        cache.Store("node1", new WelcomeFrame { ProtocolVersion = 3 }, "");
        Assert.Null(cache.Get("node1"));
        Assert.False(File.Exists(_path));
    }

    [Fact]
    public void Oversize_file_returns_null_without_deserialize()
    {
        long cap = WelcomeCache.MaxCacheFileBytes;
        var bytes = new byte[cap + 1];
        bytes[0] = (byte)'{';
        File.WriteAllBytes(_path, bytes);

        var cache = new WelcomeCache(_path);
        Assert.Null(cache.Get("us1.vrcresolver.com"));
    }

    [Fact]
    public void Oversize_file_renamed_to_oversized_marker()
    {
        long cap = WelcomeCache.MaxCacheFileBytes;
        File.WriteAllBytes(_path, new byte[cap + 1]);

        var cache = new WelcomeCache(_path);
        cache.Get("us1.vrcresolver.com");

        Assert.False(File.Exists(_path));
        var aside = Directory.GetFiles(_tempDir, "v3_welcome_cache.json.oversized-*");
        Assert.Single(aside);
        Assert.Equal(cap + 1, new FileInfo(aside[0]).Length);
    }

    [Fact]
    public void Save_failure_cleans_tmp_residue()
    {
        var cache = new WelcomeCache(_path);
        cache.Store("us1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3 }, "h0");
        Assert.True(File.Exists(_path));

        using (var lockFs = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.None))
        {
            cache.Store("us1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3 }, "h1");
        }

        Assert.False(File.Exists(_path + ".new"),
            "SaveFile catch should have deleted the .new tmp residue");
    }

    [Fact]
    public void Store_persists_welcome_hosts_fields_for_cached_hydration()
    {
        var cache = new WelcomeCache(_path);
        cache.Store("us1.vrcresolver.com", new WelcomeFrame
        {
            ProtocolVersion = 3,
            Node = "node1",
            Features = new[] { "welcome_hosts" },
            FirstPartyHosts = new[] { "vrcresolver.com", "whyknot.dev" },
            PlaybackProxyPaths = new[] { "/api/proxy", "/api/popcorn/proxy" },
        }, "hash1");

        var entry = cache.Get("us1.vrcresolver.com");
        Assert.NotNull(entry);
        Assert.Equal(new[] { "vrcresolver.com", "whyknot.dev" }, entry!.FirstPartyHosts);
        Assert.Equal(new[] { "/api/proxy", "/api/popcorn/proxy" }, entry.PlaybackProxyPaths);
    }
}
