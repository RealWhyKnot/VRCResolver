using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class LocalRelaySecurity
{
    public static bool IsAllowedHostHeader(string? hostHeader, int port)
    {
        if (string.IsNullOrWhiteSpace(hostHeader)) return false;

        string host = hostHeader.Trim();
        int colon = host.LastIndexOf(':');
        if (colon > 0 && int.TryParse(host.AsSpan(colon + 1), out int parsedPort)
            && parsedPort == port)
        {
            host = host.Substring(0, colon);
        }

        return TrustGatewayUrlBuilder.IsLocalTrustGatewayHost(host);
    }

    public static bool IsAllowedTargetUrl(string targetUrl, out string reason)
    {
        reason = "";
        if (!Uri.TryCreate(targetUrl, UriKind.Absolute, out var uri))
        {
            reason = "target_not_absolute";
            return false;
        }

        if (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps)
        {
            reason = "target_scheme_not_http";
            return false;
        }

        if (!FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUri(uri))
        {
            reason = "target_not_first_party_playback_proxy";
            return false;
        }

        return true;
    }
}
