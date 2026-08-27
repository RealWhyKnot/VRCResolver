using System.Runtime.Versioning;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class HeartbeatTests
{
    [Theory]
    [InlineData(0, "0m")]
    [InlineData(45, "0m")]
    [InlineData(60, "1m")]
    [InlineData(59 * 60, "59m")]
    [InlineData(60 * 60, "1h0m")]
    [InlineData(2 * 3600 + 13 * 60, "2h13m")]
    [InlineData(24 * 3600, "1d0h")]
    [InlineData(50 * 3600 + 30 * 60, "2d2h")]
    public void FormatUptime_ProducesExpectedShape(int seconds, string expected)
    {
        Assert.Equal(expected, Heartbeat.FormatUptime(TimeSpan.FromSeconds(seconds)));
    }

    [Theory]
    [InlineData(0, "0 B")]
    [InlineData(1023, "1023 B")]
    [InlineData(1024, "1.0 KB")]
    [InlineData(1024L * 1024, "1.0 MB")]
    [InlineData(1024L * 1024 * 1024, "1.00 GB")]
    [InlineData(1024L * 1024 * 1024 * 1024, "1.00 TB")]
    public void FormatBytes_ProducesHumanReadable(long bytes, string expected)
    {
        Assert.Equal(expected, Heartbeat.FormatBytes(bytes));
    }

    [Fact]
    public void FormatBytes_LargeRoundsToTwoDecimals()
    {
        long bytes = (long)(1.2 * 1024 * 1024 * 1024);
        Assert.Equal("1.20 GB", Heartbeat.FormatBytes(bytes));
    }
}
