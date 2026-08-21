using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

// Per-host persistent cache of v3 welcome contents, keyed by SHA256-prefix
// fingerprint the server emits in welcome.welcome_hash. On each reconnect
// the client sends client_hello carrying the cached hash; if the server's
// hash matches, it replies with the small welcome_cached frame instead of
// the full welcome — saving bytes on every reconnect.
//
// Stored at %LOCALAPPDATA%Low\vrcresolver\v3_welcome_cache.json (next to
// codec-state.json -- same LocalLow state-root convention; see
// project_locallow_state_layout.md).
//
// Per-host keying matters: current clients normally use ServerEndpoints.ProxyHost,
// while legacy clients and fallback discovery can still use node aliases.
// Single-slot would thrash across mixed deployment shapes.
//
// Thread-safety: Get / Store / Invalidate are called from the MeshClient
// run loop only — single reader, single writer. Atomic write (tmp-file +
// File.Move) so a crash mid-write leaves either old or new file intact,
// never a half-written one. Best-effort everywhere; cache miss never
// breaks the resolve hot path.
internal sealed class WelcomeCache
{
    private readonly string _path;

    public WelcomeCache()
    {
        _path = Path.Combine(AppPaths.StateRoot(), "v3_welcome_cache.json");
    }

    // Test-only constructor allowing the cache to point at a custom path
    // (per-test temp file). Production uses the parameterless ctor.
    internal WelcomeCache(string path)
    {
        _path = path;
    }

    public WelcomeCacheEntry? Get(string nodeHost)
    {
        if (string.IsNullOrEmpty(nodeHost)) return null;
        WelcomeCacheFile? file = LoadFile();
        if (file?.Nodes == null) return null;
        return file.Nodes.TryGetValue(nodeHost, out var entry) ? entry : null;
    }

    // Replace the entry for nodeHost with the contents of the freshly-
    // received welcome plus the hash the server sent. Writes the whole
    // file atomically — other nodes' entries survive.
    public void Store(string nodeHost, WelcomeFrame welcome, string hash)
    {
        if (string.IsNullOrEmpty(nodeHost) || string.IsNullOrEmpty(hash)) return;
        WelcomeCacheFile file = LoadFile() ?? new WelcomeCacheFile();
        file.Nodes ??= new Dictionary<string, WelcomeCacheEntry>(StringComparer.OrdinalIgnoreCase);
        file.Nodes[nodeHost] = new WelcomeCacheEntry
        {
            WelcomeHash = hash,
            ProtocolVersion = welcome.ProtocolVersion,
            Node = welcome.Node,
            Engines = welcome.Engines,
            Features = welcome.Features,
            WarpActive = welcome.WarpActive,
            YtDlpVersion = welcome.YtDlpVersion,
            ServerVersion = welcome.ServerVersion,
            FirstPartyHosts = welcome.FirstPartyHosts,
            PlaybackProxyPaths = welcome.PlaybackProxyPaths,
            StoredAt = DateTime.UtcNow.ToString("o"),
        };
        SaveFile(file);
    }

    // Drop the entry for nodeHost. Used when the server replies with
    // welcome_cached but our local cache slot is empty (sync drift —
    // rare; defensive).
    public void Invalidate(string nodeHost)
    {
        if (string.IsNullOrEmpty(nodeHost)) return;
        WelcomeCacheFile? file = LoadFile();
        if (file?.Nodes == null || !file.Nodes.ContainsKey(nodeHost)) return;
        file.Nodes.Remove(nodeHost);
        SaveFile(file);
    }

    // Defensive cap on the file's read size. Legitimate file is ~600 bytes
    // for a single-node cache, ~1.2 KiB for two nodes, larger only if a
    // future server adds dozens of features. 64 KiB is ~50× the realistic
    // worst case — generous enough to never bite a legitimate file, tight
    // enough that a hostile filesystem actor or unrelated corruption can't
    // induce a multi-MB JsonSerializer.Deserialize alloc before the catch
    // block fires.
    internal const long MaxCacheFileBytes = 64 * 1024;

    private WelcomeCacheFile? LoadFile()
    {
        try
        {
            if (!File.Exists(_path)) return null;
            var info = new FileInfo(_path);
            if (info.Length > MaxCacheFileBytes)
            {
                // Cache file unexpectedly large — corrupt, hostile actor,
                // or a server-side regression. Don't deserialize (would
                // pump the whole stream into S.T.J's adaptive buffer
                // before catch fires). Rename aside with a UTC marker so
                // the next launch doesn't re-trip on the same bytes; the
                // server's next welcome will rebuild a clean cache.
                Logger.WriteFileOnly("[v3-cache] oversized cache file at " + _path
                    + " (" + info.Length + " bytes; cap " + MaxCacheFileBytes
                    + ") — renaming aside, treating as cache miss");
                try
                {
                    string aside = _path + ".oversized-" + DateTime.UtcNow.ToString("yyyyMMddHHmmss");
                    File.Move(_path, aside);
                }
                catch
                { /* best-effort — if rename fails next launch
                          will hit this same branch and try again */
                }
                return null;
            }
            using var fs = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.Read);
            return JsonSerializer.Deserialize(fs, MeshJsonContext.Default.WelcomeCacheFile);
        }
        catch
        {
            // Corrupt file / permissions / etc. — pretend cache miss.
            // Next Store will overwrite; or the file persists harmlessly.
            return null;
        }
    }

    private void SaveFile(WelcomeCacheFile file)
    {
        string tmp = _path + ".new";
        try
        {
            string dir = Path.GetDirectoryName(_path)!;
            Directory.CreateDirectory(dir);
            byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(file, MeshJsonContext.Default.WelcomeCacheFile);
            File.WriteAllBytes(tmp, bytes);
            File.Move(tmp, _path, overwrite: true);
        }
        catch
        {
            // Best-effort. Cache miss next launch isn't a regression —
            // server just sends the full welcome again. Clean up any
            // tmp residue so a partial write doesn't accumulate orphan
            // .new files on disk.
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { /* best-effort */ }
        }
    }
}

// Local state-file root. Single key today; future v3.x revisions could
// add sibling top-level fields without breaking the per-node map.
internal sealed class WelcomeCacheFile
{
    [JsonPropertyName("nodes")] public Dictionary<string, WelcomeCacheEntry>? Nodes { get; set; }
}

internal sealed class WelcomeCacheEntry
{
    [JsonPropertyName("welcome_hash")] public string? WelcomeHash { get; set; }
    [JsonPropertyName("protocol_version")] public int ProtocolVersion { get; set; }
    [JsonPropertyName("node")] public string? Node { get; set; }
    [JsonPropertyName("engines")] public string[]? Engines { get; set; }
    [JsonPropertyName("features")] public string[]? Features { get; set; }
    [JsonPropertyName("warp_active")] public bool? WarpActive { get; set; }
    [JsonPropertyName("yt_dlp_version")] public string? YtDlpVersion { get; set; }
    [JsonPropertyName("server_version")] public string? ServerVersion { get; set; }
    // welcome_hosts payload. Hash-covered on the server side, so a change
    // rotates the hash and forces a full welcome -- hydrating from here on
    // welcome_cached can never serve stale lists.
    [JsonPropertyName("first_party_hosts")] public string[]? FirstPartyHosts { get; set; }
    [JsonPropertyName("playback_proxy_paths")] public string[]? PlaybackProxyPaths { get; set; }
    // Diagnostic only — not part of the wire protocol.
    [JsonPropertyName("stored_at")] public string? StoredAt { get; set; }
}
