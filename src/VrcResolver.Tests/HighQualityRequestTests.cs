using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public sealed class HighQualityRequestTests
{
    private static ResolveRequest AvProRequest() => new()
    {
        Action = WireConstants.ActionResolve,
        Id = "id-1",
        Url = "https://www.youtube.com/watch?v=x",
        Player = WireConstants.PlayerAvPro,
        MaxHeight = 1080,
        VrchatFormatArg = "bv*[height<=1080]+ba/best[height<=1080]",
    };

    [Fact]
    public void Disabled_LeavesRequestUntouched()
    {
        var req = AvProRequest();

        Assert.False(ResolveRequestProfile.ApplyHighQuality(req, enabled: false));

        Assert.Equal(1080, req.MaxHeight);
        Assert.Equal("bv*[height<=1080]+ba/best[height<=1080]", req.VrchatFormatArg);
        Assert.Null(req.PreferHighest);
    }

    [Fact]
    public void Enabled_RaisesCapAndDropsVrchatSelector()
    {
        var req = AvProRequest();

        Assert.True(ResolveRequestProfile.ApplyHighQuality(req, enabled: true));

        Assert.Equal(WireConstants.HighQualityMaxHeight, req.MaxHeight);
        Assert.Null(req.VrchatFormatArg);
        Assert.True(req.PreferHighest);
    }

    [Fact]
    public void Enabled_LeavesUnityRequestAlone()
    {
        var req = AvProRequest();
        req.Player = WireConstants.PlayerUnity;
        req.MaxHeight = 720;
        req.VrchatFormatArg = "(mp4/best)[height<=?720]";

        Assert.False(ResolveRequestProfile.ApplyHighQuality(req, enabled: true));

        Assert.Equal(720, req.MaxHeight);
        Assert.Equal("(mp4/best)[height<=?720]", req.VrchatFormatArg);
        Assert.Null(req.PreferHighest);
    }

    [Fact]
    public void RaisedRequestStillCountsAsV2()
    {
        var req = new ResolveRequest
        {
            Action = WireConstants.ActionResolve,
            Id = "id-2",
            Url = "https://www.youtube.com/watch?v=x",
            Player = WireConstants.PlayerAvPro,
        };

        Assert.True(ResolveRequestProfile.ApplyHighQuality(req, enabled: true));
        Assert.True(MeshClient.CallerOptedIntoV2(req));
    }

    [Fact]
    public void PreferHighestIsOmittedWhenUnset()
    {
        var req = AvProRequest();
        req.VrchatFormatArg = null;

        string json = System.Text.Json.JsonSerializer.Serialize(req);

        Assert.DoesNotContain("prefer_highest", json);
    }

    [Fact]
    public void PreferHighestSerializesWhenSet()
    {
        var req = AvProRequest();
        ResolveRequestProfile.ApplyHighQuality(req, enabled: true);

        string json = System.Text.Json.JsonSerializer.Serialize(req);

        Assert.Contains("\"prefer_highest\":true", json);
        Assert.Contains("\"maxHeight\":2160", json);
    }
}
