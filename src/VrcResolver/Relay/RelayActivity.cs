using System.Collections.Concurrent;
using System.Runtime.Versioning;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class RelayActivity
{
    private const int MaxTracked = 512;
    private static readonly ConcurrentDictionary<string, long> s_seen = new(StringComparer.Ordinal);

    public static void Record(string? path)
    {
        if (!TryGetPlayId(path, out string playId)) return;
        s_seen[playId] = DateTime.UtcNow.Ticks;
        if (s_seen.Count > MaxTracked) TrimOldest();
    }

    public static bool LooksWedged(string openedUrl, out int port)
    {
        port = 0;
        if (!Uri.TryCreate(openedUrl, UriKind.Absolute, out Uri? uri)) return false;
        if (!uri.Host.Equals(HostsManager.MarkerHost, StringComparison.OrdinalIgnoreCase)) return false;
        if (!TryGetPlayId(uri.AbsolutePath, out string playId)) return false;
        port = uri.Port;
        return !s_seen.ContainsKey(playId);
    }

    internal static bool TryGetPlayId(string? path, out string playId)
    {
        playId = "";
        if (path == null || !path.StartsWith("/play/", StringComparison.Ordinal)) return false;
        int end = path.IndexOf('/', 6);
        playId = end < 0 ? path[6..] : path[6..end];
        return playId.Length > 0;
    }

    private static void TrimOldest()
    {
        foreach (var entry in s_seen.OrderBy(e => e.Value).Take(s_seen.Count - MaxTracked / 2).ToList())
            s_seen.TryRemove(entry.Key, out _);
    }
}
