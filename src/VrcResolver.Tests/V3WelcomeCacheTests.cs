using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

// Per-node welcome-cache persistence. The cache file is the only state
// the v3 handshake holds across launches; a corrupt or missing file
// MUST gracefully degrade to "send null hash, take the full welcome on
// the wire" rather than throwing — the v3 wire path assumes cache is
// optional.
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
        try { Directory.Delete(_tempDir, recursive: true); } catch { /* best-effort */ }
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

        // Re-open the cache from disk to confirm the persistence layer
        // (not just an in-memory dict).
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
        // Per-node keying: writing node2's welcome must not blow away
        // node1's. Single-slot bug regression test.
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
        // Corrupt JSON should NOT throw — the v3 handshake treats it as
        // cache miss and continues.
        File.WriteAllText(_path, "{ this is not valid json");
        var cache = new WelcomeCache(_path);
        Assert.Null(cache.Get("us1.vrcresolver.com"));
    }

    [Fact]
    public void Store_writes_atomically_via_tmp_then_rename()
    {
        // After Store, no .new sidecar should remain. Earlier non-atomic
        // implementations could leave the .new file alongside on a crash;
        // confirm File.Move(overwrite:true) replaces the destination
        // cleanly.
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
        // Defensive: never persist an entry without a hash — a hashless
        // entry can't satisfy the next handshake's lookup anyway.
        var cache = new WelcomeCache(_path);
        cache.Store("node1", new WelcomeFrame { ProtocolVersion = 3 }, "");
        Assert.Null(cache.Get("node1"));
        Assert.False(File.Exists(_path));
    }

    [Fact]
    public void Oversize_file_returns_null_without_deserialize()
    {
        // Hardening: a corrupt/hostile multi-MB cache file must not
        // pump JsonSerializer.Deserialize against the whole stream
        // before catch fires. Cap is a FileInfo.Length pre-check.
        // Plant a file at MaxCacheFileBytes + 1 byte; cache must read
        // null and not throw.
        long cap = WelcomeCache.MaxCacheFileBytes;
        var bytes = new byte[cap + 1];
        // Fill with a `{` followed by zeros — would parse as the start
        // of valid JSON if we did try to deserialize. We assert the
        // cap fires BEFORE parsing.
        bytes[0] = (byte)'{';
        File.WriteAllBytes(_path, bytes);

        var cache = new WelcomeCache(_path);
        Assert.Null(cache.Get("us1.vrcresolver.com"));
    }

    [Fact]
    public void Oversize_file_renamed_to_oversized_marker()
    {
        // After the oversize check, the file gets renamed aside with a
        // .oversized-<utc> suffix so the next launch doesn't re-trip on
        // the same bytes. Server's next welcome rebuilds a clean cache.
        long cap = WelcomeCache.MaxCacheFileBytes;
        File.WriteAllBytes(_path, new byte[cap + 1]);

        var cache = new WelcomeCache(_path);
        cache.Get("us1.vrcresolver.com");

        // Original path should no longer exist (renamed aside).
        Assert.False(File.Exists(_path));
        // An .oversized-<utc> sibling should exist.
        var aside = Directory.GetFiles(_tempDir, "v3_welcome_cache.json.oversized-*");
        Assert.Single(aside);
        // And its content is the oversized bytes (preserved for forensics).
        Assert.Equal(cap + 1, new FileInfo(aside[0]).Length);
    }

    [Fact]
    public void Save_failure_cleans_tmp_residue()
    {
        // Simulate a save failure by holding the destination path open
        // with FileShare.None so File.Move can't overwrite it. SaveFile
        // catches, but its outer catch must clean up the .new tmp.
        var cache = new WelcomeCache(_path);
        // Seed a real cache file first so File.Move's destination exists.
        cache.Store("us1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3 }, "h0");
        Assert.True(File.Exists(_path));

        // Hold the cache file exclusively.
        using (var lockFs = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.None))
        {
            // Now try to Store again — File.Move should fail because the
            // destination is locked. Outer catch should delete the .new
            // tmp.
            cache.Store("us1.vrcresolver.com", new WelcomeFrame { ProtocolVersion = 3 }, "h1");
        }

        // The .new tmp should have been cleaned up.
        Assert.False(File.Exists(_path + ".new"),
            "SaveFile catch should have deleted the .new tmp residue");
    }

    [Fact]
    public void Store_persists_welcome_hosts_fields_for_cached_hydration()
    {
        // welcome_cached hydration reads these from the entry; the server's
        // hash covers them, so a change forces a full welcome and this cache
        // can never serve stale lists.
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
