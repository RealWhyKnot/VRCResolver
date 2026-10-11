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
    }

    [Theory]
    [InlineData("")]
    [InlineData("   ")]
    public void MeshWebSocketUrlRejectsBlankHost(string host)
    {
        Assert.Throws<ArgumentException>(() => ServerEndpoints.MeshWebSocketUrlForHost(host));
    }

    [Fact]
    public void MeshWebSocketUrlBuildsForAnyNodeHost()
    {
        Assert.Equal(
            "wss://us1.vrcresolver.com/mesh",
            ServerEndpoints.MeshWebSocketUrlForHost("us1.vrcresolver.com").ToString());
        Assert.Equal(
            "wss://node1.whyknot.dev/mesh",
            ServerEndpoints.MeshWebSocketUrlForHost("node1.whyknot.dev").ToString());
    }
}
