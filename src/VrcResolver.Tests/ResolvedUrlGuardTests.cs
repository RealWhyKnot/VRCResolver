using System.Net;
using System.Net.Sockets;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class ResolvedUrlGuardTests
{
    [Theory]
    [InlineData("127.0.0.1", true)]
    [InlineData("127.0.0.2", true)]
    [InlineData("0.0.0.0", true)]
    [InlineData("10.0.0.5", true)]
    [InlineData("169.254.169.254", true)]
    [InlineData("172.16.0.1", true)]
    [InlineData("172.31.255.255", true)]
    [InlineData("192.168.1.1", true)]
    [InlineData("::1", true)]
    [InlineData("fe80::1", true)]
    [InlineData("fc00::1", true)]
    [InlineData("fd12:3456::1", true)]
    [InlineData("::ffff:127.0.0.1", true)]
    [InlineData("::ffff:192.168.1.1", true)]
    [InlineData("8.8.8.8", false)]
    [InlineData("172.32.0.1", false)]
    [InlineData("2606:4700::1111", false)]
    public void BlockedAddressPolicy_BlocksLoopbackPrivateAndLinkLocal(string address, bool expected)
    {
        Assert.Equal(expected, BlockedAddressPolicy.IsBlocked(IPAddress.Parse(address)));
    }

    [Theory]
    [InlineData("https://rr3---sn-4g5e6nsz.googlevideo.com/videoplayback?expire=1", true)]
    [InlineData("https://proxy.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://vrcresolver.com/api/proxy/seg.ts?url=abc", true)]
    [InlineData("http://localhost.youtube.com:41234/play/abc/manifest.m3u8?target=eA", true)]
    [InlineData("http://93.184.216.34/video.mp4", true)]
    [InlineData("file:///C:/Windows/System32/config/SAM", false)]
    [InlineData("\\\\evil-host\\share\\video.mp4", false)]
    [InlineData("rtmp://cdn.example.com/live", false)]
    [InlineData("ftp://cdn.example.com/video.mp4", false)]
    [InlineData("http://127.0.0.1/video.mp4", false)]
    [InlineData("http://127.0.0.1:8080/video.mp4", false)]
    [InlineData("http://[::1]/video.mp4", false)]
    [InlineData("http://[::ffff:127.0.0.1]/video.mp4", false)]
    [InlineData("http://10.0.0.5/video.mp4", false)]
    [InlineData("http://169.254.169.254/latest/meta-data/", false)]
    [InlineData("http://192.168.1.1/video.mp4", false)]
    [InlineData("http://localhost/video.mp4", false)]
    [InlineData("http://LOCALHOST:9000/video.mp4", false)]
    [InlineData("http://foo.localhost/video.mp4", false)]
    [InlineData("/relative/video.mp4", false)]
    [InlineData("not a url", false)]
    [InlineData("", false)]
    [InlineData(null, false)]
    public void IsSafeToEmit_AllowsOnlyPublicHttpShapes(string? url, bool expected)
    {
        Assert.Equal(expected, ResolvedUrlGuard.IsSafeToEmit(url));
    }

    [Fact]
    public void RelayLiveness_TracksListenerLifecycle()
    {
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        int port = ((IPEndPoint)listener.LocalEndpoint).Port;
        try
        {
            Assert.True(RelayLiveness.IsListening(port));
        }
        finally
        {
            listener.Stop();
        }
        Assert.False(RelayLiveness.IsListening(port));
    }

    [Theory]
    [InlineData(0)]
    [InlineData(-1)]
    [InlineData(65536)]
    public void RelayLiveness_RejectsInvalidPorts(int port)
    {
        Assert.False(RelayLiveness.IsListening(port));
    }

    // The wrapper's AVPro backstop, now in Shared where tests reach it.
    // Blacklist semantics: trust by default, reject only the shapes AVPro
    // demonstrably cannot decode.
    [Theory]
    [InlineData("rtmp://live.example.com/stream", false)]
    [InlineData("rtmps://live.example.com/stream", false)]
    [InlineData("https://cdn.example.com/video.flv", false)]
    [InlineData("https://cdn.example.com/video.f4v", false)]
    [InlineData("https://cdn.example.com/video.flv?token=abc", false)]
    [InlineData("https://cdn.example.com/video.mp4?name=x.flv", true)]
    [InlineData("https://cdn.example.com/master.m3u8", true)]
    [InlineData("https://cdn.example.com/video.mp4", true)]
    [InlineData("", false)]
    public void IsAvProCompatibleUrl_RejectsOnlyKnownBadShapes(string url, bool expected)
    {
        Assert.Equal(expected, ResolvedUrlGuard.IsAvProCompatibleUrl(url));
    }
}
