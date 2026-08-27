using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed class ResolveCache
{
    private const int MaxEntries = 500;
    private static readonly TimeSpan ExpirySafetyMargin = TimeSpan.FromSeconds(30);
    private static readonly TimeSpan FlushDebounce = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan DefaultExpiryTtl = TimeSpan.FromMinutes(5);

    internal const long MaxCacheFileBytes = 4 * 1024 * 1024;

    private readonly string _path;
    private readonly object _lock = new();
    private ResolveCacheFile _state = new();
    private bool _loaded;
    private bool _dirty;
    private System.Threading.Timer? _flushTimer;

    public ResolveCache() : this(Path.Combine(AppPaths.StateRoot(), "resolve_cache.json")) { }

    internal ResolveCache(string path)
    {
        _path = path;
    }

    public CachedResolve? Lookup(string node, string url, string? player, string? formatArg, int? maxHeight, string requestId)
    {
        if (string.IsNullOrEmpty(url) || string.IsNullOrEmpty(node)) return null;
        EnsureLoaded();
        string key = MakeKey(node, url, player, formatArg, maxHeight);
        ResolveCacheEntry? entry;
        lock (_lock)
        {
            if (_state.Entries == null || !_state.Entries.TryGetValue(key, out entry))
                return null;
            if (!IsLive(entry.ExpiresAt))
            {
                _state.Entries.Remove(key);
                _dirty = true;
                ScheduleFlush_NoLock();
                return null;
            }
        }

        if (entry.Response == null) return null;

        var copy = CloneResponse(entry.Response);
        copy.Id = requestId;

        byte[] frame = JsonSerializer.SerializeToUtf8Bytes(copy, MeshJsonContext.Default.ResolveResponse);
        return new CachedResolve(frame, copy.Action ?? WireConstants.ActionResolved, copy.Reason);
    }

    public string? Store(string node, string url, string? player, string? formatArg, int? maxHeight, ResolveResponse response)
    {
        if (response == null) return null;
        if (!string.Equals(response.Action, WireConstants.ActionResolved, StringComparison.Ordinal)) return null;
        if (string.IsNullOrEmpty(url) || string.IsNullOrEmpty(node)) return null;

        string effectiveExpiresAt = !string.IsNullOrEmpty(response.ExpiresAt)
            ? response.ExpiresAt!
            : DateTime.UtcNow.Add(DefaultExpiryTtl).ToString("o");

        EnsureLoaded();
        string key = MakeKey(node, url, player, formatArg, maxHeight);
        string fetchedAt = DateTime.UtcNow.ToString("o");
        lock (_lock)
        {
            _state.Entries ??= new Dictionary<string, ResolveCacheEntry>(StringComparer.Ordinal);
            var clonedResp = CloneResponse(response);
            clonedResp.ExpiresAt = effectiveExpiresAt;
            _state.Entries[key] = new ResolveCacheEntry
            {
                Key = key,
                Node = node,
                Url = url,
                Player = player,
                FormatArg = formatArg,
                Response = clonedResp,
                FetchedAt = fetchedAt,
                ExpiresAt = effectiveExpiresAt,
            };
            EvictPastCap_NoLock();
            _dirty = true;
            ScheduleFlush_NoLock();
        }
        return effectiveExpiresAt;
    }

    public int EvictByUrl(string url)
    {
        if (string.IsNullOrEmpty(url)) return 0;
        EnsureLoaded();
        int removed;
        lock (_lock)
        {
            if (_state.Entries == null) return 0;
            var doomed = new List<string>();
            foreach (var kv in _state.Entries)
            {
                if (string.Equals(kv.Value.Url, url, StringComparison.Ordinal)
                    || string.Equals(kv.Value.Response?.Url, url, StringComparison.Ordinal))
                    doomed.Add(kv.Key);
            }
            foreach (string k in doomed) _state.Entries.Remove(k);
            removed = doomed.Count;
            if (removed > 0)
            {
                _dirty = true;
                ScheduleFlush_NoLock();
            }
        }
        return removed;
    }

    public bool TryGetSourceUrlForResolved(string resolvedUrl, out string sourceUrl)
    {
        sourceUrl = "";
        if (string.IsNullOrEmpty(resolvedUrl)) return false;
        EnsureLoaded();
        lock (_lock)
        {
            if (_state.Entries == null) return false;
            foreach (var kv in _state.Entries)
            {
                if (string.Equals(kv.Value.Response?.Url, resolvedUrl, StringComparison.Ordinal))
                {
                    sourceUrl = kv.Value.Url ?? "";
                    return !string.IsNullOrEmpty(sourceUrl);
                }
            }
        }
        return false;
    }

    public void FlushNow()
    {
        ResolveCacheFile? snapshot;
        lock (_lock)
        {
            if (!_dirty) return;
            snapshot = CloneState_NoLock();
            _dirty = false;
        }
        SaveFile(snapshot);
    }

    public int Count
    {
        get
        {
            EnsureLoaded();
            lock (_lock) { return _state.Entries?.Count ?? 0; }
        }
    }

    private void EnsureLoaded()
    {
        if (_loaded) return;
        lock (_lock)
        {
            if (_loaded) return;
            _state = LoadFile() ?? new ResolveCacheFile();
            if (_state.Entries != null)
            {
                var doomed = new List<string>();
                foreach (var kv in _state.Entries)
                {
                    if (!IsLive(kv.Value.ExpiresAt)) doomed.Add(kv.Key);
                }
                foreach (string k in doomed) _state.Entries.Remove(k);
                if (doomed.Count > 0) _dirty = true;
            }
            _loaded = true;
        }
    }

    private void EvictPastCap_NoLock()
    {
        if (_state.Entries == null || _state.Entries.Count <= MaxEntries) return;
        var sorted = new List<KeyValuePair<string, ResolveCacheEntry>>(_state.Entries);
        sorted.Sort((a, b) =>
            string.CompareOrdinal(a.Value.FetchedAt ?? "", b.Value.FetchedAt ?? ""));
        int toRemove = _state.Entries.Count - MaxEntries;
        for (int i = 0; i < toRemove; i++)
            _state.Entries.Remove(sorted[i].Key);
    }

    private void ScheduleFlush_NoLock()
    {
        _flushTimer ??= new System.Threading.Timer(_ => FlushTimerTick(), null, Timeout.Infinite, Timeout.Infinite);
        _flushTimer.Change(FlushDebounce, Timeout.InfiniteTimeSpan);
    }

    private void FlushTimerTick()
    {
        try { FlushNow(); }
        catch (Exception ex)
        {
            try { Logger.WriteFileOnly("[resolve-cache] flush failed: " + ex.GetType().Name + ": " + ex.Message); }
            catch { }
        }
    }

    private static bool IsLive(string? expiresAt)
    {
        if (string.IsNullOrEmpty(expiresAt)) return false;
        if (!DateTime.TryParse(expiresAt, System.Globalization.CultureInfo.InvariantCulture,
                System.Globalization.DateTimeStyles.AssumeUniversal | System.Globalization.DateTimeStyles.AdjustToUniversal,
                out var t))
            return false;
        return t > DateTime.UtcNow + ExpirySafetyMargin;
    }

    private static string MakeKey(string node, string url, string? player, string? formatArg, int? maxHeight)
    {
        const char Sep = '';
        Span<byte> hash = stackalloc byte[32];
        var sb = new StringBuilder(url.Length + (formatArg?.Length ?? 0) + node.Length + 16);
        sb.Append(node).Append(Sep);
        sb.Append(url).Append(Sep);
        sb.Append(player ?? "").Append(Sep);
        sb.Append(maxHeight?.ToString(CultureInfo.InvariantCulture) ?? "").Append(Sep);
        sb.Append(formatArg ?? "");
        byte[] bytes = Encoding.UTF8.GetBytes(sb.ToString());
        SHA256.HashData(bytes, hash);
        var hex = new StringBuilder(32);
        for (int i = 0; i < 16; i++) hex.Append(hash[i].ToString("x2"));
        return hex.ToString();
    }

    private static ResolveResponse CloneResponse(ResolveResponse r)
    {
        byte[] tmp = JsonSerializer.SerializeToUtf8Bytes(r, MeshJsonContext.Default.ResolveResponse);
        var clone = JsonSerializer.Deserialize(tmp, MeshJsonContext.Default.ResolveResponse);
        return clone ?? new ResolveResponse();
    }

    private ResolveCacheFile CloneState_NoLock()
    {
        var copy = new ResolveCacheFile { Version = _state.Version };
        if (_state.Entries != null)
        {
            copy.Entries = new Dictionary<string, ResolveCacheEntry>(_state.Entries.Count, StringComparer.Ordinal);
            foreach (var kv in _state.Entries) copy.Entries[kv.Key] = kv.Value;
        }
        return copy;
    }

    private ResolveCacheFile? LoadFile()
    {
        try
        {
            if (!File.Exists(_path)) return null;
            var info = new FileInfo(_path);
            if (info.Length > MaxCacheFileBytes)
            {
                Logger.WriteFileOnly("[resolve-cache] oversized cache file at " + _path
                    + " (" + info.Length + " bytes; cap " + MaxCacheFileBytes
                    + ") -- renaming aside, treating as cache miss");
                try
                {
                    string aside = _path + ".oversized-" + DateTime.UtcNow.ToString("yyyyMMddHHmmss");
                    File.Move(_path, aside);
                }
                catch { }
                return null;
            }
            using var fs = new FileStream(_path, FileMode.Open, FileAccess.Read, FileShare.Read);
            return JsonSerializer.Deserialize(fs, MeshJsonContext.Default.ResolveCacheFile);
        }
        catch
        {
            return null;
        }
    }

    private void SaveFile(ResolveCacheFile? file)
    {
        if (file == null) return;
        string tmp = _path + ".new";
        try
        {
            string dir = Path.GetDirectoryName(_path)!;
            Directory.CreateDirectory(dir);
            byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(file, MeshJsonContext.Default.ResolveCacheFile);
            File.WriteAllBytes(tmp, bytes);
            File.Move(tmp, _path, overwrite: true);
        }
        catch
        {
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { }
        }
    }
}

internal readonly record struct CachedResolve(byte[] Frame, string Action, string? Reason);

internal sealed class ResolveCacheFile
{
    [JsonPropertyName("version")] public int Version { get; set; } = 1;
    [JsonPropertyName("entries")] public Dictionary<string, ResolveCacheEntry>? Entries { get; set; }
}

internal sealed class ResolveCacheEntry
{
    [JsonPropertyName("key")] public string? Key { get; set; }
    [JsonPropertyName("node")] public string? Node { get; set; }
    [JsonPropertyName("url")] public string? Url { get; set; }
    [JsonPropertyName("player")] public string? Player { get; set; }
    [JsonPropertyName("format_arg")] public string? FormatArg { get; set; }
    [JsonPropertyName("response")] public ResolveResponse? Response { get; set; }
    [JsonPropertyName("fetched_at")] public string? FetchedAt { get; set; }
    [JsonPropertyName("expires_at")] public string? ExpiresAt { get; set; }
}
