using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

// Per-(url, player, format, node) persistent cache of server `resolved`
// frames keyed by SHA256-prefix fingerprint. Replays the cached
// ResolveResponse to the wrapper directly, skipping the WS round-trip
// and server-side lookup on repeat URLs whose server-issued
// `expires_at` is still in the future.
//
// File: %LOCALAPPDATA%Low\vrcresolver\resolve_cache.json (next to
// v3_welcome_cache.json / codec-state.json -- same LocalLow state-root).
//
// Decision rules:
//   * Cache only on terminal `resolved` action with non-null ExpiresAt.
//     `fallback_native` is unstable, never cached. Missing ExpiresAt
//     from server = no staleness signal -> don't cache.
//   * 30 s safety margin on hit: treat expires_at < now + 30s as expired,
//     fall through to mesh.
//   * Per-node keying: include negotiated mesh node in the key. Different
//     nodes can have different resolver configs; never cross-serve.
//   * Cap at 500 entries; evict oldest fetched_at on insert past cap.
//
// Persistence:
//   * In-memory dict authoritative on the hot path.
//   * Debounced 5 s flush + flush-on-shutdown via FlushNow().
//   * Atomic write (tmp + rename) so crash mid-write leaves either old
//     or new file intact, never half-written.
//
// Eviction triggers:
//   * Past cap (500 entries) on insert: drop oldest by fetched_at.
//   * Expired on lookup: skipped + dropped lazily.
//   * VrcLogMonitor.silent_stall: caller invokes EvictByUrl when AVPro
//     fell silent on a URL we just served, closing the staleness loop
//     without server help.
//   * Corrupt file at load: treat as empty, continue. File rebuilds.
//
// Failure modes (all degrade gracefully):
//   * Stale URL served (server clock skew) -> AVPro 403/404 -> wrapper
//     og fallback. silent_stall watchdog evicts the key.
//   * Cache file corrupt -> load returns empty, hot path keeps working.
//   * Disk full on flush -> log warn, in-memory state persists.
//
// Thread-safety: all public methods take the same lock. Hot-path Lookup
// is sub-microsecond against an in-memory Dictionary, so the lock
// contention is theoretical (resolves are bounded by IPC + WS latency,
// not cache lookup).
internal sealed class ResolveCache
{
    private const int MaxEntries = 500;
    private static readonly TimeSpan ExpirySafetyMargin = TimeSpan.FromSeconds(30);
    private static readonly TimeSpan FlushDebounce = TimeSpan.FromSeconds(5);
    // Fallback TTL applied when the server's `resolved` frame omits
    // expires_at. Most CDN-issued URLs (googlevideo, m3u8 playlists)
    // last hours, so 5 min is conservative -- short enough that
    // staleness is rare, long enough to coalesce a flurry of repeat
    // resolves from VRChat instance loads where many players spawn at
    // once. The 30-second safety margin still applies on top, so a
    // cache hit older than (5 min - 30 s) falls through to mesh.
    //
    // Tracked in [resolve-cache] log lines as "expires_at_default".
    // When server starts emitting expires_at on every resolve (single-
    // line server change), this fallback can be removed.
    private static readonly TimeSpan DefaultExpiryTtl = TimeSpan.FromMinutes(5);

    // Defensive cap on the file's read size. Worst-case legitimate file
    // at MaxEntries=500 with ~1.5 KiB per entry (resolved frame +
    // metadata) is ~750 KiB. Cap at 4 MiB so a hostile filesystem
    // actor or unrelated corruption can't induce a multi-MB
    // JsonSerializer.Deserialize alloc before catch fires.
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

    // Returns the cached frame ready to write to the pipe (with the
    // request's id stamped in), or null on cache miss / expired entry.
    public CachedResolve? Lookup(string node, string url, string? player, string? formatArg, string requestId)
    {
        if (string.IsNullOrEmpty(url) || string.IsNullOrEmpty(node)) return null;
        EnsureLoaded();
        string key = MakeKey(node, url, player, formatArg);
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

        // Stamp the current request's id onto the cached response so the
        // watchdog audit log + future server-side checks line up.
        var copy = CloneResponse(entry.Response);
        copy.Id = requestId;

        byte[] frame = JsonSerializer.SerializeToUtf8Bytes(copy, MeshJsonContext.Default.ResolveResponse);
        return new CachedResolve(frame, copy.Action ?? WireConstants.ActionResolved, copy.Reason);
    }

    // Cache a fresh server `resolved` response. No-op for fallback_native
    // (unstable) or empty url. Missing expires_at applies the fallback
    // DefaultExpiryTtl so the cache still yields value when the server
    // omits the field; the 30-second safety margin still applies on hit.
    //
    // Returns the effective expires_at written to the entry, or null if
    // the response was rejected. Diagnostic only -- callers don't need
    // to inspect.
    public string? Store(string node, string url, string? player, string? formatArg, ResolveResponse response)
    {
        if (response == null) return null;
        if (!string.Equals(response.Action, WireConstants.ActionResolved, StringComparison.Ordinal)) return null;
        if (string.IsNullOrEmpty(url) || string.IsNullOrEmpty(node)) return null;

        // If server omits expires_at, synthesize one from
        // DefaultExpiryTtl. Most CDN-issued URLs last hours; 5 minutes
        // is conservatively short.
        string effectiveExpiresAt = !string.IsNullOrEmpty(response.ExpiresAt)
            ? response.ExpiresAt!
            : DateTime.UtcNow.Add(DefaultExpiryTtl).ToString("o");

        EnsureLoaded();
        string key = MakeKey(node, url, player, formatArg);
        string fetchedAt = DateTime.UtcNow.ToString("o");
        lock (_lock)
        {
            _state.Entries ??= new Dictionary<string, ResolveCacheEntry>(StringComparer.Ordinal);
            var clonedResp = CloneResponse(response);
            // Persist the effective expires_at on the response too so a
            // future replay always reports a coherent timestamp.
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

    // Drop every entry whose source URL or resolved playback URL matches
    // across all (node, player, format) combinations. VrcLogMonitor calls
    // this when AVPro fell silent on a URL we just served; with the trust
    // gateway, VRChat logs the localhost URL, which is canonicalized back
    // to the resolved playback URL before eviction.
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

    // Reverse lookup: given a resolved playback URL that VRChat reported
    // failing, return the original source URL the user / world handed to
    // the wrapper. VrcLogMonitor uses this to mark the source for the
    // reactive og-fallback hint so the next wrapper call short-circuits.
    // Returns the first match; if multiple cache entries point at the
    // same resolved URL they share the source (the cache key includes
    // node/player/format but the source URL is constant per content).
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

    // Synchronous flush for graceful shutdown.
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

    // Current in-memory entry count. Surfaced on the periodic Heartbeat
    // line as "cache=N" so the operator can see how many resolves are
    // serveable from disk without going to mesh.
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
            // Prune already-expired entries so a long-stopped watchdog
            // doesn't resurrect stale URLs against AVPro on next launch.
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
            catch { /* ignore */ }
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

    // SHA256(node + 0x1F + url + 0x1F + player + 0x1F + format) -> hex of
    // the first 16 bytes (32 hex chars). 0x1F (Unit Separator) is an
    // ASCII control char that cannot appear unencoded in URLs, player
    // tokens, or yt-dlp -f selectors -- prevents an adversarial url
    // containing a literal separator from colliding with a different
    // (url, format) combination.
    private static string MakeKey(string node, string url, string? player, string? formatArg)
    {
        const char Sep = '';
        Span<byte> hash = stackalloc byte[32];
        var sb = new StringBuilder(url.Length + (formatArg?.Length ?? 0) + node.Length + 16);
        sb.Append(node).Append(Sep);
        sb.Append(url).Append(Sep);
        sb.Append(player ?? "").Append(Sep);
        sb.Append(formatArg ?? "");
        byte[] bytes = Encoding.UTF8.GetBytes(sb.ToString());
        SHA256.HashData(bytes, hash);
        var hex = new StringBuilder(32);
        for (int i = 0; i < 16; i++) hex.Append(hash[i].ToString("x2"));
        return hex.ToString();
    }

    // Cheap deep copy via source-gen round-trip -- preserves
    // [JsonExtensionData] unknown fields the server included that we
    // don't have typed properties for.
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
                catch { /* best-effort */ }
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
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { /* best-effort */ }
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
