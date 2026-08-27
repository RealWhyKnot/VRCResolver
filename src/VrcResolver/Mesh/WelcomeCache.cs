using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed class WelcomeCache
{
    private readonly string _path;

    public WelcomeCache()
    {
        _path = Path.Combine(AppPaths.StateRoot(), "v3_welcome_cache.json");
    }

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

    public void Invalidate(string nodeHost)
    {
        if (string.IsNullOrEmpty(nodeHost)) return;
        WelcomeCacheFile? file = LoadFile();
        if (file?.Nodes == null || !file.Nodes.ContainsKey(nodeHost)) return;
        file.Nodes.Remove(nodeHost);
        SaveFile(file);
    }

    internal const long MaxCacheFileBytes = 64 * 1024;

    private WelcomeCacheFile? LoadFile()
    {
        try
        {
            if (!File.Exists(_path)) return null;
            var info = new FileInfo(_path);
            if (info.Length > MaxCacheFileBytes)
            {
                Logger.WriteFileOnly("[v3-cache] oversized cache file at " + _path
                    + " (" + info.Length + " bytes; cap " + MaxCacheFileBytes
                    + ") — renaming aside, treating as cache miss");
                try
                {
                    string aside = _path + ".oversized-" + DateTime.UtcNow.ToString("yyyyMMddHHmmss");
                    File.Move(_path, aside);
                }
                catch
                {
                }
                return null;
            }
            using var fs = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.Read);
            return JsonSerializer.Deserialize(fs, MeshJsonContext.Default.WelcomeCacheFile);
        }
        catch
        {
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
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { }
        }
    }
}

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
    [JsonPropertyName("first_party_hosts")] public string[]? FirstPartyHosts { get; set; }
    [JsonPropertyName("playback_proxy_paths")] public string[]? PlaybackProxyPaths { get; set; }
    [JsonPropertyName("stored_at")] public string? StoredAt { get; set; }
}
