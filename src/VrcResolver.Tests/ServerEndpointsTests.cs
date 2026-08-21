using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public class ServerEndpointsTests
{
    [Fact]
    public void DefaultsUseProxyPublicHost()
    {
        Assert.Equal("vrcresolver.com", ServerEndpoints.ProxyHost);
        Assert.Equal(
            "wss://vrcresolver.com/mesh",
            ServerEndpoints.MeshWebSocketUrlForHost(ServerEndpoints.ProxyHost).ToString());
        Assert.Equal("https://vrcresolver.com/api/report", ServerEndpoints.ReportUrl.ToString());
        Assert.Equal("https://vrcresolver.com/", ServerEndpoints.ApexDiscoveryUrl.ToString());
    }

    [Theory]
    [InlineData("https://us1.vrcresolver.com/", true, "us1.vrcresolver.com")]
    [InlineData("https://eu1.vrcresolver.com/mesh", true, "eu1.vrcresolver.com")]
    [InlineData("https://node1.whyknot.dev/", true, "node1.whyknot.dev")]
    [InlineData("https://proxy.whyknot.dev/", true, "proxy.whyknot.dev")]
    [InlineData("https://vrcresolver.com/", false, "vrcresolver.com")]
    [InlineData("/mesh", false, "vrcresolver.com")]
    [InlineData("https://evil.example.com/", false, "evil.example.com")]
    [InlineData("https://vrcresolver.com.evil.com/", false, "vrcresolver.com.evil.com")]
    [InlineData("https://203.0.113.7/", false, "203.0.113.7")]
    [InlineData("https://127.0.0.1/", false, "127.0.0.1")]
    [InlineData("https://[::1]/", false, "[::1]")]
    public void DiscoveryRedirectHostRequiresFirstPartyDnsHost(
        string location,
        bool expected,
        string expectedHost)
    {
        bool ok = ServerEndpoints.TryExtractDiscoveryRedirectHost(new Uri(location, UriKind.RelativeOrAbsolute), out string host);

        Assert.Equal(expected, ok);
        Assert.Equal(expectedHost, host);
    }

    [Fact]
    public void MeshWebSocketUrlBuildsForAnyNodeHost()
    {
        Assert.Equal(
            "wss://us1.vrcresolver.com/mesh",
            ServerEndpoints.MeshWebSocketUrlForHost("us1.vrcresolver.com").ToString());
        // Discovery may still hand back a node under the pre-rename server
        // domain; the mesh URL builder is host-agnostic.
        Assert.Equal(
            "wss://node1.whyknot.dev/mesh",
            ServerEndpoints.MeshWebSocketUrlForHost("node1.whyknot.dev").ToString());
    }
}
