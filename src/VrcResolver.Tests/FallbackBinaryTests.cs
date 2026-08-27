using System.IO;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public sealed class FallbackBinaryTests
{
    private const string Exe = @"C:\vrc\dist\tools";
    private const string Vrc = @"C:\Users\u\AppData\LocalLow\VRChat\VRChat\Tools";

    private static string Og(string dir) => Path.Combine(dir, "yt-dlp-og.exe");
    private static string YtDlp(string dir) => Path.Combine(dir, "yt-dlp.exe");

    [Fact]
    public void Select_PrefersOgNextToWrapper()
    {
        var picked = FallbackBinary.Select(Exe, Vrc,
            exists: p => p == Og(Exe) || p == Og(Vrc),
            isOurWrapper: _ => false);
        Assert.Equal(Og(Exe), picked);
    }

    [Fact]
    public void Select_FallsBackToVrcToolsOg_WhenLocalOgMissing()
    {
        var picked = FallbackBinary.Select(Exe, Vrc,
            exists: p => p == Og(Vrc),
            isOurWrapper: _ => false);
        Assert.Equal(Og(Vrc), picked);
    }

    [Fact]
    public void Select_FallsBackToVrcVanillaYtDlp_WhenNoOgAnywhere()
    {
        var picked = FallbackBinary.Select(Exe, Vrc,
            exists: p => p == YtDlp(Vrc),
            isOurWrapper: _ => false);
        Assert.Equal(YtDlp(Vrc), picked);
    }

    [Fact]
    public void Select_SkipsCandidatesThatAreOurWrapper()
    {
        var picked = FallbackBinary.Select(Exe, Vrc,
            exists: p => p == Og(Exe) || p == YtDlp(Vrc),
            isOurWrapper: p => p == Og(Exe));
        Assert.Equal(YtDlp(Vrc), picked);
    }

    [Fact]
    public void Select_ReturnsNull_WhenNothingExists()
        => Assert.Null(FallbackBinary.Select(Exe, Vrc, exists: _ => false, isOurWrapper: _ => false));

    [Fact]
    public void Select_ReturnsNull_WhenOnlyCandidateIsOurWrapper()
    {
        Assert.Null(FallbackBinary.Select(Exe, Vrc,
            exists: p => p == YtDlp(Vrc),
            isOurWrapper: _ => true));
    }

    [Fact]
    public void Select_HandlesNullVrcToolsDir()
    {
        Assert.Equal(Og(Exe),
            FallbackBinary.Select(Exe, null, exists: p => p == Og(Exe), isOurWrapper: _ => false));
        Assert.Null(
            FallbackBinary.Select(Exe, null, exists: _ => false, isOurWrapper: _ => false));
    }
}
