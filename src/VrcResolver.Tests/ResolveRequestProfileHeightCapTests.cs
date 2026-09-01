using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class ResolveRequestProfileHeightCapTests
{
    private const string VrchatArg = "(mp4/best)[height<=?2160][height>=?64][width>=?64]";

    [Fact]
    public void CapHeightIn_lowers_the_cap_and_leaves_the_rest_alone()
        => Assert.Equal("(mp4/best)[height<=?1080][height>=?64][width>=?64]",
            ResolveRequestProfile.CapHeightIn(VrchatArg, 1080));

    [Fact]
    public void CapHeightIn_leaves_a_lower_cap_untouched()
    {
        const string arg = "(mp4/best)[height<=?720][height>=?64]";
        Assert.Same(arg, ResolveRequestProfile.CapHeightIn(arg, 1080));
    }

    [Fact]
    public void CapHeightIn_handles_the_unprefixed_form_and_several_clauses()
        => Assert.Equal("bv[height<=1080]/bv[height<=1080]/bv[height<=480]",
            ResolveRequestProfile.CapHeightIn("bv[height<=4320]/bv[height<=1440]/bv[height<=480]", 1080));

    [Fact]
    public void ApplyDefaultQualityCap_caps_avpro_requests_in_both_places()
    {
        var req = new ResolveRequest
        {
            Player = WireConstants.PlayerAvPro,
            MaxHeight = 2160,
            VrchatFormatArg = VrchatArg,
        };
        Assert.True(ResolveRequestProfile.ApplyDefaultQualityCap(req));
        Assert.Equal(1080, req.MaxHeight);
        Assert.Equal("(mp4/best)[height<=?1080][height>=?64][width>=?64]", req.VrchatFormatArg);
    }

    [Fact]
    public void ApplyDefaultQualityCap_never_raises_a_lower_request()
    {
        var req = new ResolveRequest { Player = WireConstants.PlayerAvPro, MaxHeight = 480 };
        Assert.False(ResolveRequestProfile.ApplyDefaultQualityCap(req));
        Assert.Equal(480, req.MaxHeight);
    }

    [Fact]
    public void ApplyDefaultQualityCap_leaves_unity_alone()
    {
        var req = new ResolveRequest
        {
            Player = WireConstants.PlayerUnity,
            MaxHeight = 720,
            VrchatFormatArg = "(mp4/best)[height<=?720]",
        };
        Assert.False(ResolveRequestProfile.ApplyDefaultQualityCap(req));
        Assert.Equal(720, req.MaxHeight);
        Assert.Equal("(mp4/best)[height<=?720]", req.VrchatFormatArg);
    }

    [Fact]
    public void ApplyDefaultQualityCap_fills_in_the_default_when_vrchat_sent_no_cap()
    {
        var req = new ResolveRequest { Player = WireConstants.PlayerAvPro };
        Assert.True(ResolveRequestProfile.ApplyDefaultQualityCap(req));
        Assert.Equal(1080, req.MaxHeight);
    }

    [Fact]
    public void ApplyHighQuality_still_wins_over_the_cap()
    {
        var req = new ResolveRequest
        {
            Player = WireConstants.PlayerAvPro,
            MaxHeight = 2160,
            VrchatFormatArg = VrchatArg,
        };
        Assert.True(ResolveRequestProfile.ApplyHighQuality(req, enabled: true));
        Assert.Equal(WireConstants.HighQualityMaxHeight, req.MaxHeight);
        Assert.Null(req.VrchatFormatArg);
    }
}
