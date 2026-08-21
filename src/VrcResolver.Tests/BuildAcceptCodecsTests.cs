using System.Collections.Generic;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

// The accept_codecs claim must stay a multi-entry set and stay TRUE: the
// baseline (h264/aac) and audio codecs always survive, and extension-backed
// video codecs appear only when the capability probe verified a decoder.
public class BuildAcceptCodecsTests
{
    private static HashSet<string> Set(params string[] items) => new(items);

    [Fact]
    public void AllVerified_YieldsTheFullAvProList_InOrder()
    {
        Assert.Equal(WireConstants.AvProAcceptCodecs,
            WireConstants.BuildAcceptCodecs(Set("h265", "vp9", "av1")));
    }

    [Fact]
    public void NothingVerified_KeepsBaselineAndAudio()
    {
        Assert.Equal(new[] { "h264", "aac", "opus", "mp3", "ac3", "eac3" },
            WireConstants.BuildAcceptCodecs(Set()));
    }

    [Fact]
    public void ProbeUnavailable_IsTreatedLikeNothingVerified()
    {
        Assert.Equal(new[] { "h264", "aac", "opus", "mp3", "ac3", "eac3" },
            WireConstants.BuildAcceptCodecs(null));
    }

    [Fact]
    public void PartialVerification_PrunesOnlyTheUnverified()
    {
        Assert.Equal(new[] { "h264", "h265", "aac", "opus", "mp3", "ac3", "eac3" },
            WireConstants.BuildAcceptCodecs(Set("h265")));
    }

    [Fact]
    public void ExtensionBackedList_MatchesTheInstallerTargets()
    {
        Assert.Equal(new[] { "h265", "vp9", "av1" }, WireConstants.ExtensionBackedVideoCodecs);
    }
}
