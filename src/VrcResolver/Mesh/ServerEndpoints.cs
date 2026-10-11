namespace VrcResolver;

internal static class ServerEndpoints
{
    public const string ProxyHost = "vrcresolver.com";
    public static readonly Uri ReportUrl = new("https://vrcresolver.com/api/report");

    public static Uri MeshWebSocketUrlForHost(string host)
    {
        host = (host ?? "").Trim();
        if (host.Length == 0)
            throw new ArgumentException("mesh host is required", nameof(host));

        return new Uri("wss://" + host + "/mesh");
    }
}
