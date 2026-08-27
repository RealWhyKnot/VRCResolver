using System.Runtime.Versioning;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class HostsManagerTests
{
    [Theory]
    [InlineData("127.0.0.1 localhost.youtube.com", true)]
    [InlineData("127.0.0.1\tlocalhost.youtube.com", true)]
    [InlineData("127.0.0.1   localhost.youtube.com   # vrcresolver", true)]
    [InlineData("127.0.0.1   localhost.youtube.com   # WKVRCProxy", true)]
    [InlineData("127.0.0.1 localhost.youtube.com  # any other comment", true)]
    [InlineData("    127.0.0.1 localhost.youtube.com", true)]
    [InlineData("127.0.0.1 some.other.host localhost.youtube.com extra.host", true)]
    [InlineData("127.0.0.1 LocalHost.YouTube.com", true)]
    public void LineIsBypassEntry_RecognizesValidEntries(string line, bool expected)
    {
        Assert.Equal(expected, HostsManager.LineIsBypassEntry(line));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("   ")]
    [InlineData("\t\t")]
    public void LineIsBypassEntry_RejectsBlankLines(string? line)
    {
        Assert.False(HostsManager.LineIsBypassEntry(line));
    }

    [Theory]
    [InlineData("# 127.0.0.1 localhost.youtube.com")]
    [InlineData("    # 127.0.0.1 localhost.youtube.com")]
    [InlineData("#127.0.0.1 localhost.youtube.com")]
    public void LineIsBypassEntry_IgnoresComments(string line)
    {
        Assert.False(HostsManager.LineIsBypassEntry(line));
    }

    [Theory]
    [InlineData("127.0.0.2 localhost.youtube.com")]
    [InlineData("0.0.0.0 localhost.youtube.com")]
    [InlineData("192.168.1.1 localhost.youtube.com")]
    [InlineData("::1 localhost.youtube.com")]
    public void LineIsBypassEntry_RejectsWrongIp(string line)
    {
        Assert.False(HostsManager.LineIsBypassEntry(line));
    }

    [Theory]
    [InlineData("127.0.0.1 notlocalhost.youtube.com")]
    [InlineData("127.0.0.1 localhost.youtube.com.evil.com")]
    [InlineData("127.0.0.1 prefixlocalhost.youtube.com")]
    public void LineIsBypassEntry_RejectsSubstringMatches(string line)
    {
        Assert.False(HostsManager.LineIsBypassEntry(line));
    }

    [Theory]
    [InlineData("127.0.0.1")]
    [InlineData("localhost.youtube.com")]
    [InlineData("127.0.0.1 example.com")]
    [InlineData("# WKVRCProxy hosts entry follows")]
    public void LineIsBypassEntry_RejectsMalformedOrUnrelated(string line)
    {
        Assert.False(HostsManager.LineIsBypassEntry(line));
    }

    [Fact]
    public void LineIsBypassEntry_StripsTrailingComment()
    {
        Assert.True(HostsManager.LineIsBypassEntry("127.0.0.1 localhost.youtube.com#WKVRCProxy"));
        Assert.False(HostsManager.LineIsBypassEntry("127.0.0.1 #localhost.youtube.com"));
    }
}
