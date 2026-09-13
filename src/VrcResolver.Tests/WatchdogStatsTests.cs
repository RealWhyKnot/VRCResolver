using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

public class WatchdogStatsTests
{
    [Fact]
    public void RecordRelayBytes_CountsUpstreamTargetsSeparately()
    {
        WatchdogStats.ResetForTests();

        WatchdogStats.RecordRelayRequest("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc");
        WatchdogStats.RecordRelayBytes("https://us1.vrcresolver.com/api/proxy/segment.ts?q=abc", 100);
        WatchdogStats.RecordRelayBytes("https://example.com/video.ts", 25);

        WatchdogActivitySnapshot snapshot = WatchdogStats.GetActivitySnapshot();

        Assert.Equal(1, snapshot.RelayRequestsTotal);
        Assert.Equal(125, snapshot.RelayBytesTotal);
        Assert.Equal(1, snapshot.UpstreamRelayRequestsTotal);
        Assert.Equal(100, snapshot.UpstreamRelayBytesTotal);
        Assert.True(snapshot.LastRelayUtc.HasValue);
        Assert.True(snapshot.LastUpstreamRelayUtc.HasValue);
    }

    [Fact]
    public void RecordRelayBytes_PrecomputedFlagMatchesPerUrlClassification()
    {
        WatchdogStats.ResetForTests();

        string upstream = "https://us1.vrcresolver.com/api/proxy/segment.ts";
        Assert.True(WatchdogStats.ClassifyUpstreamTarget(upstream));
        Assert.False(WatchdogStats.ClassifyUpstreamTarget("https://example.com/video.ts"));

        WatchdogStats.RecordRelayBytes(upstream, true, 100);
        WatchdogStats.RecordRelayBytes("https://example.com/video.ts", false, 25);

        WatchdogActivitySnapshot snapshot = WatchdogStats.GetActivitySnapshot();
        Assert.Equal(125, snapshot.RelayBytesTotal);
        Assert.Equal(100, snapshot.UpstreamRelayBytesTotal);
    }

    [Fact]
    public void ActivitySnapshot_ReportsRecentRelayActivity()
    {
        var now = DateTime.UtcNow;
        var snapshot = new WatchdogActivitySnapshot(
            ResolvesTotal: 0,
            ResolvesViaLhYt: 0,
            ResolvesCacheHits: 0,
            BytesEstimateTotal: 0,
            ReconnectCount: 0,
            RelayRequestsTotal: 1,
            RelayBytesTotal: 50,
            UpstreamRelayRequestsTotal: 1,
            UpstreamRelayBytesTotal: 50,
            LastRelayUtc: now - TimeSpan.FromMilliseconds(100),
            LastUpstreamRelayUtc: now - TimeSpan.FromMilliseconds(100));

        Assert.True(snapshot.RelayActive(now, TimeSpan.FromSeconds(1)));
        Assert.True(snapshot.UpstreamActive(now, TimeSpan.FromSeconds(1)));
        Assert.False(snapshot.RelayActive(now + TimeSpan.FromSeconds(2), TimeSpan.FromSeconds(1)));
    }

    [Fact]
    public void BandwidthSnapshot_ReportsRecentBytesBySecond()
    {
        WatchdogStats.ResetForTests();
        var now = new DateTime(2026, 5, 10, 12, 0, 10, DateTimeKind.Utc);

        WatchdogStats.RecordRelayBytesAt("https://us1.vrcresolver.com/api/proxy/a.ts", 1000, now - TimeSpan.FromSeconds(2));
        WatchdogStats.RecordRelayBytesAt("https://us1.vrcresolver.com/api/proxy/b.ts", 2000, now);
        WatchdogStats.RecordRelayBytesAt("https://us1.vrcresolver.com/api/proxy/c.ts", 3000, now);

        WatchdogBandwidthSnapshot bandwidth = WatchdogStats.GetBandwidthSnapshot(now, seconds: 4);

        Assert.Equal(new long[] { 0, 1000, 0, 5000 }, bandwidth.HistoryBytesPerSecond);
        Assert.Equal(5000, bandwidth.CurrentBytesPerSecond);
        Assert.Equal(5000, bandwidth.PeakBytesPerSecond);
        Assert.True(bandwidth.HasTraffic);
    }
}
