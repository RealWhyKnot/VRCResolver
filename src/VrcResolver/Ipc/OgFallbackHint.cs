using System.Collections.Concurrent;

namespace VrcResolver;

// Reactive (not predictive) signal that one or more recent AVPro load_failure
// events implicate a specific source URL. While the hint is live for that
// source, the IPC handler short-circuits new wrapper resolves to fallback_native
// so the user's next retry execs yt-dlp-og.exe directly instead of getting the
// same broken playback URL back from the mesh.
//
// This is NOT a blocklist of "bad sources". Entries:
//   * are populated ONLY by an observed VRChat-side failure on a URL we
//     ourselves served (the trigger is the failure event, not heuristics),
//   * expire after a short TTL so the mesh path gets a fresh shot on the
//     next play attempt without operator intervention,
//   * carry no scoring, no consecutive-failure counter, no persistence.
//
// Without a hint, every wrapper call still goes through the mesh normally.
internal sealed class OgFallbackHint
{
    // Long enough to cover a user mashing Play several times after a
    // failure, short enough that a transient upstream blip doesn't
    // permanently shunt the source to native. 60 s is the same window
    // VrcLogMonitor uses to decide silent_stall, which keeps the
    // failure -> fallback -> recovery cycle predictable.
    public static readonly TimeSpan DefaultTtl = TimeSpan.FromSeconds(60);

    private readonly ConcurrentDictionary<string, DateTime> _expiresUtc = new(StringComparer.Ordinal);
    private readonly TimeSpan _ttl;
    private readonly Func<DateTime> _now;

    public OgFallbackHint() : this(DefaultTtl, () => DateTime.UtcNow) { }

    // Test seam: deterministic TTL + clock injection.
    internal OgFallbackHint(TimeSpan ttl, Func<DateTime> nowUtc)
    {
        _ttl = ttl;
        _now = nowUtc;
    }

    public TimeSpan Ttl => _ttl;

    public void RecordLoadFailure(string sourceUrl)
    {
        if (string.IsNullOrEmpty(sourceUrl)) return;
        _expiresUtc[sourceUrl] = _now() + _ttl;
    }

    // Returns true if a recent failure is still in-window for this source.
    // Stale entries are dropped on the read path so the dictionary doesn't
    // grow without bound between sweeps.
    public bool ShouldPreferOg(string sourceUrl)
    {
        if (string.IsNullOrEmpty(sourceUrl)) return false;
        if (!_expiresUtc.TryGetValue(sourceUrl, out DateTime expires)) return false;
        if (expires > _now()) return true;
        _expiresUtc.TryRemove(new KeyValuePair<string, DateTime>(sourceUrl, expires));
        return false;
    }

    public bool TryClear(string sourceUrl)
    {
        if (string.IsNullOrEmpty(sourceUrl)) return false;
        return _expiresUtc.TryRemove(sourceUrl, out _);
    }

    public int LiveEntryCountForTests()
    {
        int n = 0;
        DateTime now = _now();
        foreach (var kv in _expiresUtc)
            if (kv.Value > now) n++;
        return n;
    }
}
