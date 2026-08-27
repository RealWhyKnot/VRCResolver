using System.Text.Json;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public class UpdateCheckPrereleaseTests
{
    [Fact]
    public void PickHighestFromList_PicksHighestVersion_NotMostRecentlyPublished()
    {
        string json = """
        [
          { "tag_name": "v2026.6.0.0-beta", "html_url": "https://example.test/beta", "prerelease": true },
          { "tag_name": "v2026.5.10.5",      "html_url": "https://example.test/p5",   "prerelease": false }
        ]
        """;
        using var doc = JsonDocument.Parse(json);

        var pick = UpdateCheck.PickHighestFromList(doc.RootElement);

        Assert.NotNull(pick);
        Assert.Equal("v2026.6.0.0-beta", pick!.Value.tag);
        Assert.True(pick.Value.isPrerelease);
    }

    [Fact]
    public void PickHighestFromList_PrefersStableOverPrereleaseOnVersionTie()
    {
        string json = """
        [
          { "tag_name": "v2026.5.10.0-pre1", "html_url": "https://example.test/pre1", "prerelease": true },
          { "tag_name": "v2026.5.10.0",      "html_url": "https://example.test/p0",   "prerelease": false }
        ]
        """;
        using var doc = JsonDocument.Parse(json);

        var pick = UpdateCheck.PickHighestFromList(doc.RootElement);

        Assert.NotNull(pick);
        Assert.Equal("v2026.5.10.0", pick!.Value.tag);
        Assert.False(pick.Value.isPrerelease);
    }

    [Fact]
    public void PickHighestFromList_StableEntryStaysWinnerEvenWhenPrereleaseAppearsLast()
    {
        string json = """
        [
          { "tag_name": "v2026.5.10.0",      "html_url": "https://example.test/p0",   "prerelease": false },
          { "tag_name": "v2026.5.10.0-pre1", "html_url": "https://example.test/pre1", "prerelease": true }
        ]
        """;
        using var doc = JsonDocument.Parse(json);

        var pick = UpdateCheck.PickHighestFromList(doc.RootElement);

        Assert.NotNull(pick);
        Assert.Equal("v2026.5.10.0", pick!.Value.tag);
        Assert.False(pick.Value.isPrerelease);
    }

    [Fact]
    public void PickHighestFromList_ReturnsNullForEmptyArray()
    {
        string json = "[]";
        using var doc = JsonDocument.Parse(json);

        Assert.Null(UpdateCheck.PickHighestFromList(doc.RootElement));
    }

    [Fact]
    public void PickHighestFromList_SkipsEntriesWithUnparsableTags()
    {
        string json = """
        [
          { "tag_name": "garbage-tag", "html_url": "x", "prerelease": false },
          { "tag_name": "v2026.5.10.0", "html_url": "y", "prerelease": false }
        ]
        """;
        using var doc = JsonDocument.Parse(json);

        var pick = UpdateCheck.PickHighestFromList(doc.RootElement);

        Assert.NotNull(pick);
        Assert.Equal("v2026.5.10.0", pick!.Value.tag);
    }

    [Fact]
    public void PickHighestFromList_StripsTrailingDevSuffixBeforeComparing()
    {
        string json = """
        [
          { "tag_name": "v2026.5.10.0-AAAA", "html_url": "x", "prerelease": true },
          { "tag_name": "v2026.5.9.5",        "html_url": "y", "prerelease": false }
        ]
        """;
        using var doc = JsonDocument.Parse(json);

        var pick = UpdateCheck.PickHighestFromList(doc.RootElement);

        Assert.NotNull(pick);
        Assert.Equal("v2026.5.10.0-AAAA", pick!.Value.tag);
    }
}
