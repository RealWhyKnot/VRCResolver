using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public class OgFallbackHintTests
{
    [Fact]
    public void ShouldPreferOg_DefaultsFalse_WhenSourceNeverFailed()
    {
        var clock = new TestClock(DateTime.UtcNow);
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        Assert.False(hint.ShouldPreferOg("https://www.youtube.com/watch?v=abc"));
    }

    [Fact]
    public void ShouldPreferOg_TrueWithinTtl_AfterRecordedFailure()
    {
        var clock = new TestClock(new DateTime(2026, 5, 22, 10, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        hint.RecordLoadFailure("https://www.youtube.com/watch?v=abc");

        Assert.True(hint.ShouldPreferOg("https://www.youtube.com/watch?v=abc"));
        clock.Advance(TimeSpan.FromSeconds(30));
        Assert.True(hint.ShouldPreferOg("https://www.youtube.com/watch?v=abc"));
    }

    [Fact]
    public void ShouldPreferOg_FalseAfterTtl_DroppedFromMap()
    {
        var clock = new TestClock(new DateTime(2026, 5, 22, 10, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        hint.RecordLoadFailure("https://www.youtube.com/watch?v=abc");
        clock.Advance(TimeSpan.FromSeconds(61));

        Assert.False(hint.ShouldPreferOg("https://www.youtube.com/watch?v=abc"));
        Assert.Equal(0, hint.LiveEntryCountForTests());
    }

    [Fact]
    public void RecordLoadFailure_ReArmsExpiry_OnRepeatedFailure()
    {
        var clock = new TestClock(new DateTime(2026, 5, 22, 10, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        hint.RecordLoadFailure("https://www.youtube.com/watch?v=abc");
        clock.Advance(TimeSpan.FromSeconds(45));
        hint.RecordLoadFailure("https://www.youtube.com/watch?v=abc");

        clock.Advance(TimeSpan.FromSeconds(45));
        Assert.True(hint.ShouldPreferOg("https://www.youtube.com/watch?v=abc"));
    }

    [Fact]
    public void RecordLoadFailure_IsKeyedBySourceUrl_NotResolvedUrl()
    {
        var clock = new TestClock(new DateTime(2026, 5, 22, 10, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        hint.RecordLoadFailure("https://www.youtube.com/watch?v=abc");

        Assert.False(hint.ShouldPreferOg("https://www.youtube.com/watch?v=def"));
        Assert.True(hint.ShouldPreferOg("https://www.youtube.com/watch?v=abc"));
    }

    [Fact]
    public void TryClear_RemovesActiveEntry()
    {
        var clock = new TestClock(new DateTime(2026, 5, 22, 10, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        hint.RecordLoadFailure("https://www.youtube.com/watch?v=abc");
        Assert.True(hint.TryClear("https://www.youtube.com/watch?v=abc"));
        Assert.False(hint.ShouldPreferOg("https://www.youtube.com/watch?v=abc"));
        Assert.False(hint.TryClear("https://www.youtube.com/watch?v=abc"));
    }

    [Fact]
    public void RecordLoadFailure_EmptyOrNullSourceUrl_NoOp()
    {
        var clock = new TestClock(DateTime.UtcNow);
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        hint.RecordLoadFailure("");
        hint.RecordLoadFailure(null!);

        Assert.Equal(0, hint.LiveEntryCountForTests());
        Assert.False(hint.ShouldPreferOg(""));
    }

    [Fact]
    public void OgBlock_CoversEveryYouTubeHostAndOnlyYouTube()
    {
        var clock = new TestClock(new DateTime(2026, 9, 29, 18, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        Assert.True(hint.RecordOgBlocked("https://youtu.be/abc"));

        Assert.True(hint.IsOgBlocked("https://www.youtube.com/watch?v=def"));
        Assert.True(hint.IsOgBlocked("https://music.youtube.com/watch?v=ghi"));
        Assert.True(hint.IsOgBlocked("https://m.youtube.com/watch?v=jkl"));
        Assert.False(hint.IsOgBlocked("https://soundcloud.com/a/b"));
        Assert.False(hint.IsOgBlocked("https://notyoutube.com/v"));
    }

    [Fact]
    public void OgBlock_OverridesThePerVideoHintUntilItExpires()
    {
        var clock = new TestClock(new DateTime(2026, 9, 29, 18, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromHours(1), clock.Now);

        hint.RecordLoadFailure("https://youtu.be/abc");
        hint.RecordOgBlocked("https://youtu.be/abc");
        Assert.False(hint.ShouldPreferOg("https://youtu.be/abc"));

        clock.Advance(OgFallbackHint.OgBlockedTtl + TimeSpan.FromSeconds(1));
        Assert.False(hint.IsOgBlocked("https://youtu.be/abc"));
        Assert.True(hint.ShouldPreferOg("https://youtu.be/abc"));
    }

    [Fact]
    public void RecordOgBlocked_ReportsOnlyTheFirstBlockAndRefreshesExpiry()
    {
        var clock = new TestClock(new DateTime(2026, 9, 29, 18, 0, 0, DateTimeKind.Utc));
        var hint = new OgFallbackHint(TimeSpan.FromSeconds(60), clock.Now);

        Assert.True(hint.RecordOgBlocked("https://youtu.be/abc"));
        clock.Advance(OgFallbackHint.OgBlockedTtl - TimeSpan.FromMinutes(1));
        Assert.False(hint.RecordOgBlocked("https://www.youtube.com/watch?v=def"));
        clock.Advance(TimeSpan.FromMinutes(2));
        Assert.True(hint.IsOgBlocked("https://youtu.be/abc"));
    }

    [Theory]
    [InlineData("")]
    [InlineData("not a url")]
    [InlineData("file:///C:/video.mp4")]
    public void RecordOgBlocked_IgnoresUrlsWithoutAHost(string url)
    {
        var hint = new OgFallbackHint();

        Assert.False(hint.RecordOgBlocked(url));
        Assert.False(hint.IsOgBlocked(url));
    }

    [Fact]
    public void DefaultTtl_IsSixtySeconds()
    {
        var hint = new OgFallbackHint();
        Assert.Equal(TimeSpan.FromSeconds(60), hint.Ttl);
    }

    private sealed class TestClock
    {
        private DateTime _now;
        public TestClock(DateTime initialUtc) => _now = initialUtc;
        public DateTime Now() => _now;
        public void Advance(TimeSpan by) => _now += by;
    }
}
