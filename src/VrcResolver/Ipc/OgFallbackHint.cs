using System.Collections.Concurrent;

namespace VrcResolver;

internal sealed class OgFallbackHint
{
    public static readonly TimeSpan DefaultTtl = TimeSpan.FromSeconds(60);

    private readonly ConcurrentDictionary<string, DateTime> _expiresUtc = new(StringComparer.Ordinal);
    private readonly TimeSpan _ttl;
    private readonly Func<DateTime> _now;

    public OgFallbackHint() : this(DefaultTtl, () => DateTime.UtcNow) { }

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
