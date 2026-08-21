using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class FirstPartyUrlPolicyTests
{
    // Both host families are first-party: the server intentionally keeps
    // returning whyknot-family playback URLs for wire compatibility with
    // pre-rename clients.
    [Theory]
    [InlineData("https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://proxy.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://proxy.whyknot.dev/api/proxy/seg.ts?url=abc&wkedge=node1", true)]
    [InlineData("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://whyknot.dev/api/proxy/lazy-hls/wk_abc/index.m3u8", true)]
    [InlineData("https://node2.whyknot.dev/api/popcorn/proxy/manifest.m3u8?clientId=c&index=1", true)]
    [InlineData("https://node2.whyknot.dev/api/popcorn/proxy/seg.ts?clientId=c&url=abc", true)]
    [InlineData("https://node1.whyknot.dev/api/restream/shared-status", false)]
    [InlineData("https://example.com/api/proxy/manifest.m3u8?q=abc", false)]
    [InlineData("https://notwhyknot.dev/api/proxy/manifest.m3u8?q=abc", false)]
    [InlineData("https://evil-whyknot.dev/api/proxy/manifest.m3u8?q=abc", false)]
    [InlineData("https://evil-vrcresolver.com/api/proxy/manifest.m3u8?q=abc", false)]
    [InlineData("https://vrcresolver.com.evil.com/api/proxy/manifest.m3u8?q=abc", false)]
    [InlineData("not a url", false)]
    public void IsFirstPartyPlaybackProxyUrl_AcceptsOnlyFirstPartyPlaybackProxyShapes(
        string url,
        bool expected)
    {
        Assert.Equal(expected, FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUrl(url));
    }

    [Theory]
    [InlineData("vrcresolver.com", true)]
    [InlineData("us1.vrcresolver.com", true)]
    [InlineData("whyknot.dev", true)]
    [InlineData("node1.whyknot.dev", true)]
    [InlineData("evil-whyknot.dev", false)]
    [InlineData("vrcresolver.com.evil.com", false)]
    [InlineData("example.com", false)]
    public void IsFirstPartyHost_AcceptsBothHostFamilies(string host, bool expected)
    {
        Assert.Equal(expected, FirstPartyUrlPolicy.IsFirstPartyHost(host));
    }

    [Theory]
    [InlineData("https://proxy.whyknot.dev/api/proxy/manifest.m3u8?q=abc", "m3u8")]
    [InlineData("https://node1.whyknot.dev/api/proxy/manifest.mpd?q=abc", "mpd")]
    [InlineData("https://node1.whyknot.dev/api/proxy?q=legacy", "m3u8")]
    [InlineData("https://node1.whyknot.dev/api/popcorn/proxy/manifest.m3u8?clientId=c&index=1", "m3u8")]
    [InlineData("https://cdn.example.com/video/seg.ts?sig=abc", "ts")]
    [InlineData("https://node1.whyknot.dev/api/proxy?url=legacy", "bin")]
    [InlineData("https://cdn.example.com/video/no-extension?sig=abc", "")]
    [InlineData("not a url", "")]
    public void PlaybackProxyExtensionForTrustGateway_KeepsAvproDispatchExtension(
        string url,
        string expected)
    {
        Assert.Equal(expected, FirstPartyUrlPolicy.PlaybackProxyExtensionForTrustGateway(url));
    }

    [Fact]
    public void TrustGatewayUrlBuilder_WrapsOnlyWhyKnotPlaybackProxyTargets()
    {
        const string target = "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc";
        bool ok = TrustGatewayUrlBuilder.TryBuild(
            51234,
            target,
            "test-session",
            out string localUrl);

        Assert.True(ok);
        Assert.StartsWith(
            "http://localhost.youtube.com:51234/play/testsession/manifest.m3u8?target=",
            localUrl);
        Assert.True(TrustGatewayUrlBuilder.TryExtractTarget(localUrl, out string extracted));
        Assert.Equal(target, extracted);

        Assert.False(TrustGatewayUrlBuilder.TryBuild(
            51234,
            "https://cdn.example.com/video/seg.ts",
            "test-session",
            out _));

        Assert.False(TrustGatewayUrlBuilder.TryBuild(
            51234,
            "http://localhost.youtube.com:51234/play/a/manifest.m3u8?target=x",
            "test-session",
            out _));

        Assert.True(TrustGatewayUrlBuilder.TryBuild(
            51234,
            "https://node1.whyknot.dev/api/proxy/manifest.m3u8?note=localhost.youtube.com",
            "test-session",
            out _));
    }

    [Fact]
    public void TrustGatewayUrlBuilder_CanEmitHttpsGatewayUrls()
    {
        const string target = "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc";

        bool ok = TrustGatewayUrlBuilder.TryBuild(
            51234,
            target,
            "test-session",
            "https",
            out string localUrl);

        Assert.True(ok);
        Assert.StartsWith(
            "https://localhost.youtube.com:51234/play/testsession/manifest.m3u8?target=",
            localUrl);
        Assert.True(TrustGatewayUrlBuilder.TryExtractTarget(localUrl, out string extracted));
        Assert.Equal(target, extracted);
    }

    [Theory]
    [InlineData("http", true)]
    [InlineData("https", true)]
    [InlineData("HTTPS", true)]
    [InlineData("ftp", false)]
    [InlineData("", false)]
    public void TrustGatewayUrlBuilder_AllowsOnlyHttpAndHttpsGatewaySchemes(string scheme, bool expected)
    {
        Assert.Equal(expected, TrustGatewayUrlBuilder.IsAllowedGatewayScheme(scheme));
    }

    [Theory]
    [InlineData("localhost.youtube.com", true)]
    [InlineData("LOCALHOST.YOUTUBE.COM", true)]
    [InlineData("localhost", true)]
    [InlineData("127.0.0.1", true)]
    [InlineData("notlocalhost.youtube.com", false)]
    [InlineData("localhost.youtube.com.evil.com", false)]
    [InlineData("youtube.com", false)]
    public void TrustGatewayUrlBuilder_LocalGatewayHostListIsExact(string host, bool expected)
    {
        Assert.Equal(expected, TrustGatewayUrlBuilder.IsLocalTrustGatewayHost(host));
    }

    [Fact]
    public void TrustGatewayUrlBuilder_ExtractTargetRejectsNonGatewayUrls()
    {
        Assert.False(TrustGatewayUrlBuilder.TryExtractTarget(
            "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc",
            out _));
        Assert.False(TrustGatewayUrlBuilder.TryExtractTarget(
            "http://localhost.youtube.com:51234/play/a/manifest.m3u8?target=not-base64",
            out _));
    }
}
