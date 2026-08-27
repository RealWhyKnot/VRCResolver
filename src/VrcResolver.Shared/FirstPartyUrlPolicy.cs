namespace VrcResolver.Shared;

public static class FirstPartyUrlPolicy
{
    private static readonly HashSet<string> s_allowedPathExts = new(StringComparer.OrdinalIgnoreCase)
    {
        "mp4", "m4s", "m4v", "ts", "m3u8", "mpd",
        "webm", "mkv", "mov",
        "mp3", "m4a", "aac", "ogg", "opus", "wav", "flac",
        "vtt", "srt",
    };

    private static string[] s_serverHostFamilies = Array.Empty<string>();
    private static string[] s_serverProxyPaths = Array.Empty<string>();

    public const int MaxServerProvidedEntries = 8;

    public static void SetServerProvided(string[]? hostFamilies, string[]? proxyPaths)
    {
        s_serverHostFamilies = SanitizeHostFamilies(hostFamilies);
        s_serverProxyPaths = SanitizeProxyPaths(proxyPaths);
    }

    internal static void ResetServerProvidedForTests() => SetServerProvided(null, null);

    private static string[] SanitizeHostFamilies(string[]? entries)
    {
        if (entries == null || entries.Length == 0) return Array.Empty<string>();
        var result = new List<string>(Math.Min(entries.Length, MaxServerProvidedEntries));
        foreach (var raw in entries)
        {
            if (result.Count >= MaxServerProvidedEntries) break;
            if (string.IsNullOrWhiteSpace(raw)) continue;
            string host = raw.Trim().TrimEnd('.').ToLowerInvariant();
            if (host.Length is < 4 or > 253) continue;
            if (System.Net.IPAddress.TryParse(host, out _)) continue;
            var labels = host.Split('.');
            if (labels.Length < 2) continue;
            bool ok = true;
            foreach (var label in labels)
            {
                if (label.Length is < 1 or > 63) { ok = false; break; }
                foreach (var c in label)
                {
                    if (c is not ((>= 'a' and <= 'z') or (>= '0' and <= '9') or '-')) { ok = false; break; }
                }
                if (!ok) break;
            }
            if (ok) result.Add(host);
        }
        return result.ToArray();
    }

    private static string[] SanitizeProxyPaths(string[]? entries)
    {
        if (entries == null || entries.Length == 0) return Array.Empty<string>();
        var result = new List<string>(Math.Min(entries.Length, MaxServerProvidedEntries));
        foreach (var raw in entries)
        {
            if (result.Count >= MaxServerProvidedEntries) break;
            if (string.IsNullOrWhiteSpace(raw)) continue;
            string path = raw.Trim();
            if (path.Length is < 6 or > 64) continue;
            if (!path.StartsWith("/api/", StringComparison.Ordinal)) continue;
            if (path.EndsWith("/", StringComparison.Ordinal)) continue;
            if (path.Contains("..", StringComparison.Ordinal) || path.Contains("//", StringComparison.Ordinal)) continue;
            bool ok = true;
            foreach (var c in path)
            {
                if (c is not ((>= 'a' and <= 'z') or (>= 'A' and <= 'Z') or (>= '0' and <= '9') or '/' or '-' or '_')) { ok = false; break; }
            }
            if (ok) result.Add(path);
        }
        return result.ToArray();
    }

    public static bool IsFirstPartyHost(string host)
    {
        if (host.Equals("vrcresolver.com", StringComparison.OrdinalIgnoreCase)
            || host.EndsWith(".vrcresolver.com", StringComparison.OrdinalIgnoreCase)
            || host.Equals("whyknot.dev", StringComparison.OrdinalIgnoreCase)
            || host.EndsWith(".whyknot.dev", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }
        foreach (var family in s_serverHostFamilies)
        {
            if (host.Equals(family, StringComparison.OrdinalIgnoreCase)
                || host.EndsWith("." + family, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }
        return false;
    }

    public static bool IsFirstPartyPlaybackProxyUrl(string url)
    {
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri)) return false;
        return IsFirstPartyPlaybackProxyUri(uri);
    }

    public static bool IsFirstPartyPlaybackProxyUri(Uri uri)
    {
        return IsFirstPartyHost(uri.Host) && IsPlaybackProxyPath(uri.AbsolutePath);
    }

    public static string ExtractAllowedPathExtension(string url)
    {
        if (string.IsNullOrEmpty(url)) return "";
        try
        {
            if (!Uri.TryCreate(url, UriKind.Absolute, out var uri)) return "";
            return ExtractAllowedPathExtensionFromPath(uri.AbsolutePath);
        }
        catch
        {
            return "";
        }
    }

    public static string ExtractAllowedPathExtensionFromPath(string path)
    {
        if (string.IsNullOrEmpty(path)) return "";
        string ext = Path.GetExtension(path);
        if (string.IsNullOrEmpty(ext) || ext.Length < 2) return "";
        string trimmed = ext.Substring(1).ToLowerInvariant();
        return s_allowedPathExts.Contains(trimmed) ? trimmed : "";
    }

    public static string PlaybackProxyExtensionForTrustGateway(string url)
    {
        string ext = ExtractAllowedPathExtension(url);
        if (!string.IsNullOrEmpty(ext)) return ext;

        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri)
            || !IsFirstPartyPlaybackProxyUri(uri))
        {
            return "";
        }

        string path = uri.AbsolutePath;
        if (path.Equals("/api/proxy", StringComparison.OrdinalIgnoreCase)
            && uri.Query.Contains("q=", StringComparison.OrdinalIgnoreCase))
        {
            return "m3u8";
        }

        if (path.Contains("manifest", StringComparison.OrdinalIgnoreCase))
            return "m3u8";

        return "bin";
    }

    private static bool IsPlaybackProxyPath(string path)
    {
        if (path.Equals("/api/proxy", StringComparison.OrdinalIgnoreCase)
            || path.StartsWith("/api/proxy/", StringComparison.OrdinalIgnoreCase)
            || path.Equals("/api/popcorn/proxy", StringComparison.OrdinalIgnoreCase)
            || path.StartsWith("/api/popcorn/proxy/", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }
        foreach (var prefix in s_serverProxyPaths)
        {
            if (path.Equals(prefix, StringComparison.OrdinalIgnoreCase)
                || path.StartsWith(prefix + "/", StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }
        return false;
    }
}
