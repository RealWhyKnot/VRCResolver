using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

internal readonly record struct LocalRelayTarget(string Url, string Kind);

[SupportedOSPlatform("windows")]
internal sealed class LocalRelayTargetResolver
{
    private const int RelativeBaseSoftCap = 256;
    private const int RelativeBaseHardCap = 512;

    private readonly object _gate = new();
    private readonly Dictionary<string, string> _relativeBases = new(StringComparer.OrdinalIgnoreCase);
    private readonly Queue<string> _relativeBaseOrder = new();

    public bool TryResolve(string path, string query, string? targetParam, out LocalRelayTarget target)
    {
        target = default;
        if (!string.IsNullOrEmpty(targetParam))
        {
            string targetUrl = DecodeTargetParam(targetParam);
            if (LocalRelaySecurity.IsAllowedTargetUrl(targetUrl, out _))
                RememberRelativeBase(path, targetUrl);
            target = new LocalRelayTarget(targetUrl, "target");
            return true;
        }

        if (TryResolveRelativeRequest(path, query, out string relativeTarget))
        {
            target = new LocalRelayTarget(relativeTarget, "relative");
            return true;
        }

        return false;
    }

    private void RememberRelativeBase(string localPath, string targetUrl)
    {
        if (string.IsNullOrEmpty(targetUrl)) return;
        if (!Uri.TryCreate(targetUrl, UriKind.Absolute, out var targetUri)) return;

        string localPrefix = LocalPrefixForPath(localPath);
        string upstreamBase = new Uri(targetUri, ".").ToString();
        lock (_gate)
        {
            if (!_relativeBases.ContainsKey(localPrefix))
                _relativeBaseOrder.Enqueue(localPrefix);

            _relativeBases[localPrefix] = upstreamBase;
            while (_relativeBases.Count > RelativeBaseHardCap
                && _relativeBaseOrder.Count > 0)
            {
                string old = _relativeBaseOrder.Dequeue();
                if (_relativeBases.Count <= RelativeBaseSoftCap) break;
                _relativeBases.Remove(old);
            }
        }
    }

    private bool TryResolveRelativeRequest(string path, string query, out string targetUrl)
    {
        targetUrl = "";
        string bestPrefix = "";
        string bestBase = "";
        lock (_gate)
        {
            foreach (var kvp in _relativeBases)
            {
                if (kvp.Key.Length > bestPrefix.Length
                    && path.StartsWith(kvp.Key, StringComparison.OrdinalIgnoreCase))
                {
                    bestPrefix = kvp.Key;
                    bestBase = kvp.Value;
                }
            }
        }

        if (string.IsNullOrEmpty(bestPrefix) || string.IsNullOrEmpty(bestBase))
            return false;

        return TryResolveRelativeTarget(path, query, bestPrefix, bestBase, out targetUrl);
    }

    internal static string LocalPrefixForPath(string path)
    {
        if (!IsPlayPath(path))
            return "/play/";
        if (string.Equals(path, "/play", StringComparison.OrdinalIgnoreCase))
            return "/play/";
        if (path.EndsWith("/", StringComparison.Ordinal)) return path;

        int slash = path.LastIndexOf('/');
        if (slash < 0) return "/play/";
        return path.Substring(0, slash + 1);
    }

    internal static bool TryResolveRelativeTarget(
        string localPath,
        string query,
        string localPrefix,
        string upstreamBase,
        out string targetUrl)
    {
        targetUrl = "";
        if (string.IsNullOrEmpty(localPath)
            || string.IsNullOrEmpty(localPrefix)
            || string.IsNullOrEmpty(upstreamBase)
            || !localPath.StartsWith(localPrefix, StringComparison.OrdinalIgnoreCase)
            || !Uri.TryCreate(upstreamBase, UriKind.Absolute, out var baseUri))
        {
            return false;
        }

        string relative = localPath.Substring(localPrefix.Length);
        if (string.IsNullOrEmpty(relative)) return false;
        if (!string.IsNullOrEmpty(query)) relative += query;
        if (!Uri.TryCreate(baseUri, relative, out var resolved)) return false;
        targetUrl = resolved.ToString();
        return true;
    }

    internal static bool IsPlayPath(string path)
    {
        return string.Equals(path, "/play", StringComparison.OrdinalIgnoreCase)
            || path.StartsWith("/play/", StringComparison.OrdinalIgnoreCase);
    }

    private static string DecodeTargetParam(string targetParam)
        => Base64UrlText.TryDecode(targetParam, out string decoded) ? decoded : "";

    public static string EncodeTargetParam(string url) => Base64UrlText.Encode(url);
}
