using VrcResolver.Shared;

namespace VrcResolver;

internal static class ServerEndpoints
{
    public const string ProxyHost = "vrcresolver.com";
    public static readonly Uri ApexDiscoveryUrl = new("https://vrcresolver.com/");
    public static readonly Uri ReportUrl = new("https://vrcresolver.com/api/report");

    public static Uri MeshWebSocketUrlForHost(string host)
    {
        host = (host ?? "").Trim();
        if (host.Length == 0)
            throw new ArgumentException("mesh host is required", nameof(host));

        return new Uri("wss://" + host + "/mesh");
    }

    public static bool TryExtractDiscoveryRedirectHost(Uri location, out string host)
    {
        var baseUri = ApexDiscoveryUrl;
        Uri absolute = location.IsAbsoluteUri ? location : new Uri(baseUri, location);
        host = absolute.Host;
        return host.Length > 0
            && absolute.HostNameType == UriHostNameType.Dns
            && FirstPartyUrlPolicy.IsFirstPartyHost(host)
            && !host.Equals(baseUri.Host, StringComparison.OrdinalIgnoreCase);
    }
}
