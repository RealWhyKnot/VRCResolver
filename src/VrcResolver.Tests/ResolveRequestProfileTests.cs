using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public sealed class ResolveRequestProfileTests
{
    [Theory]
    [InlineData("(mp4/best)[height<=?720]", 720)]
    [InlineData("best[height<=1080]/best", 1080)]
    [InlineData("bv*[protocol^=m3u8_native][height<=?480]+ba", 480)]
    public void TryGetHeightCap_ParsesVrchatSelectorCaps(string formatArg, int expected)
    {
        Assert.Equal(expected, ResolveRequestProfile.TryGetHeightCap(formatArg));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("best")]
    [InlineData("best[height<=?]")]
    public void TryGetHeightCap_ReturnsNullWhenNoNumericCap(string? formatArg)
    {
        Assert.Null(ResolveRequestProfile.TryGetHeightCap(formatArg));
    }

    [Theory]
    [InlineData("(mp4/best)[height<=?720]", WireConstants.PlayerUnity)]
    [InlineData("best[height<=720]/best", WireConstants.PlayerUnity)]
    [InlineData("best[height<=1080]/best", WireConstants.PlayerAvPro)]
    [InlineData(null, WireConstants.PlayerAvPro)]
    public void InferPlayer_TreatsOptional720CapAsUnity(string? formatArg, string expected)
    {
        Assert.Equal(expected, ResolveRequestProfile.InferPlayer(formatArg));
    }

    [Fact]
    public void ExtractDashFValue_MatchesSpacedFormsOnly()
    {
        Assert.Equal("best[height<=1080]",
            ResolveRequestProfile.ExtractDashFValue(new[] { "-f", "best[height<=1080]", "https://x" }));
        Assert.Equal("best",
            ResolveRequestProfile.ExtractDashFValue(new[] { "--format", "best", "https://x" }));
        Assert.Null(ResolveRequestProfile.ExtractDashFValue(new[] { "https://x" }));
        Assert.Null(ResolveRequestProfile.ExtractDashFValue(new[] { "https://x", "-f" }));
        Assert.Null(ResolveRequestProfile.ExtractDashFValue(new[] { "--format=best", "https://x" }));
    }
}
