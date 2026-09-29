using System.Collections.Concurrent;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed class OgFallbackHint
{
    public static readonly TimeSpan DefaultTtl = TimeSpan.FromSeconds(60);
    public static readonly TimeSpan OgBlockedTtl = TimeSpan.FromMinutes(30);

    private readonly ConcurrentDictionary<string, DateTime> _expiresUtc = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, DateTime> _ogBlockedUntilUtc = new(StringComparer.Ordinal);
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
        if (string.IsNullOrEmpty(sourceUrl) || IsOgBlocked(sourceUrl)) return false;
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

    public bool RecordOgBlocked(string sourceUrl)
    {
        string site = SiteKey(sourceUrl);
        if (site.Length == 0) return false;
        bool wasBlocked = IsOgBlocked(sourceUrl);
        _ogBlockedUntilUtc[site] = _now() + OgBlockedTtl;
        return !wasBlocked;
    }

    public bool IsOgBlocked(string sourceUrl)
    {
        string site = SiteKey(sourceUrl);
        if (site.Length == 0 || !_ogBlockedUntilUtc.TryGetValue(site, out DateTime until)) return false;
        if (until > _now()) return true;
        _ogBlockedUntilUtc.TryRemove(new KeyValuePair<string, DateTime>(site, until));
        return false;
    }

    internal static string SiteKey(string? url)
    {
        string host = LogUtil.BareHost(url).ToLowerInvariant();
        if (host == "?") return "";
        if (host is "youtu.be" or "youtube.com" or "youtube-nocookie.com"
            || host.EndsWith(".youtube.com", StringComparison.Ordinal))
        {
            return "youtube.com";
        }
        return host;
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
