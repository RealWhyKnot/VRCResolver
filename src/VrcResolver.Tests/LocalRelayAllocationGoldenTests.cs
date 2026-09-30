using System.Runtime.Versioning;
using System.Security.Cryptography;
using System.Text;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class LocalRelayAllocationGoldenTests
{
    private static string Proxy(string url, string name)
    {
        string normalized = new Uri(url).ToString();
        string ns = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(normalized)), 0, 8).ToLowerInvariant();
        string b64 = Convert.ToBase64String(Encoding.UTF8.GetBytes(normalized)).Replace('+', '-').Replace('/', '_').TrimEnd('=');
        return "proxy/" + ns + "/" + name + "?target=" + b64;
    }

    private const string Seg1 = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=tok";
    private const string Seg2 = "https://eu1.vrcresolver.com/api/proxy/seg.ts?a=1&b=2";
    private const string Variant = "https://proxy.whyknot.dev/api/proxy/a/index.m3u8?x=1";
    private const string Key = "https://eu1.vrcresolver.com/api/proxy/key.bin?clientId=c";
    private const string Map = "https://us1.vrcresolver.com/api/proxy/init~mp4?q=%C3%A9";
    private const string Media = "https://us1.vrcresolver.com/api/proxy/audio/en.m3u8?q=z";

    private static string BuildInput(string nl) => string.Join(nl, new[]
    {
        "#EXTM3U",
        "#EXT-X-VERSION:6",
        "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"" + Media + "\"",
        "#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720",
        Variant,
        "",
        "#EXT-X-KEY:METHOD=AES-128,URI=\"" + Key + "\",IV=0x01",
        "#EXT-X-MAP:URI=\"" + Map + "\"",
        "#EXTINF:2.000,",
        Seg1,
        "#EXTINF:2.000,",
        "   " + Seg2.Replace("&", "&amp;") + "  ",
        "#EXTINF:2.000,",
        "seg_relative_3.ts",
        "#EXTINF:2.000,",
        "https://cdn.example.com/video/seg.ts?x=1",
        "data:text/plain;base64,AAAA",
        "\t",
        "#EXT-X-ENDLIST",
    }) + nl;

    private static string BuildExpected() => string.Join("\n", new[]
    {
        "#EXTM3U",
        "#EXT-X-VERSION:6",
        "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"" + Proxy(Media, "en.m3u8") + "\"",
        "#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720",
        Proxy(Variant, "index.m3u8"),
        "",
        "#EXT-X-KEY:METHOD=AES-128,URI=\"" + Proxy(Key, "key.bin") + "\",IV=0x01",
        "#EXT-X-MAP:URI=\"" + Proxy(Map, "proxy.bin") + "\"",
        "#EXTINF:2.000,",
        Proxy(Seg1, "seg_000001.ts"),
        "#EXTINF:2.000,",
        "   " + Proxy(Seg2, "seg.ts") + "  ",
        "#EXTINF:2.000,",
        "seg_relative_3.ts",
        "#EXTINF:2.000,",
        "https://cdn.example.com/video/seg.ts?x=1",
        "data:text/plain;base64,AAAA",
        "\t",
        "#EXT-X-ENDLIST",
    }) + "\n";

    [Theory]
    [InlineData("\n")]
    [InlineData("\r\n")]
    public async Task ManifestLocalizer_GoldenOutput(string nl)
    {
        string input = BuildInput(nl);
        string expected = BuildExpected();
        string expectedTrimmed = expected.Substring(0, expected.Length - 1);

        Assert.Equal(expectedTrimmed, LocalRelayManifestLocalizer.Localize(input, "/play/abc/manifest.m3u8"));

        using var ms = new MemoryStream(Encoding.UTF8.GetBytes(input));
        using var output = new MemoryStream();
        var result = await LocalRelayManifestLocalizer.LocalizeStreamAsync(
            ms,
            inputEncoding: null,
            output,
            LocalRelayManifestLocalizer.MaxManifestBytes,
            CancellationToken.None);

        Assert.True(result.Changed);
        Assert.False(result.Exceeded);
        Assert.Equal(expectedTrimmed, Encoding.UTF8.GetString(output.ToArray()));
        Assert.Equal(expectedTrimmed.Length, result.CharsOut);
    }

    [Fact]
    public async Task ManifestLocalizer_UnchangedManifestIsReportedUnchanged()
    {
        string input = "#EXTM3U\n#EXTINF:2,\nseg1.ts\n\n";
        using var ms = new MemoryStream(Encoding.UTF8.GetBytes(input));
        using var output = new MemoryStream();
        var result = await LocalRelayManifestLocalizer.LocalizeStreamAsync(
            ms,
            inputEncoding: null,
            output,
            LocalRelayManifestLocalizer.MaxManifestBytes,
            CancellationToken.None);

        Assert.False(result.Changed);
        Assert.Equal("#EXTM3U\n#EXTINF:2,\nseg1.ts\n", Encoding.UTF8.GetString(output.ToArray()));
    }

    [Fact]
    public async Task ManifestLocalizer_StopsWhenOutputExceedsCap()
    {
        string input = "#EXTM3U\n#EXTINF:2,\nseg1.ts\nseg2.ts\n";
        using var ms = new MemoryStream(Encoding.UTF8.GetBytes(input));
        using var output = new MemoryStream();
        var result = await LocalRelayManifestLocalizer.LocalizeStreamAsync(
            ms,
            inputEncoding: null,
            output,
            maxChars: 12,
            CancellationToken.None);

        Assert.True(result.Exceeded);
        Assert.Equal("#EXTM3U\n#EXTINF:2,", Encoding.UTF8.GetString(output.ToArray()));
    }

    private static LocalRelayTimingSample Sample(
        string method,
        string localPath,
        string target,
        int status,
        long header,
        long total,
        long wait = -1,
        string? failure = null)
        => new(method, localPath, target, status, header, total, 100, "HIT", wait, null, failure);

    private static readonly DateTime s_now = new(2026, 9, 30, 12, 0, 0, DateTimeKind.Utc);

    [Fact]
    public void HitchDetector_GoldenAnalyze()
    {
        LocalRelayHitchDetector.ResetForTests();
        string lazy = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000010.ts";

        Assert.Null(LocalRelayHitchDetector.AnalyzeForTests(Sample("POST", "/a/seg.ts", lazy, 500, 9000, 9000), s_now));
        Assert.Null(LocalRelayHitchDetector.AnalyzeForTests(Sample("GET", "/a/x.bin", "https://h/x.bin", 500, 9000, 9000), s_now));
        Assert.Null(LocalRelayHitchDetector.AnalyzeForTests(Sample("GET", "/a/seg.ts", "https://h/p/seg.ts", 200, 10, 20), s_now));
        Assert.Null(LocalRelayHitchDetector.AnalyzeForTests(Sample("get", "/a/m.m3u8", "https://h/p/m.m3u8", 200, 1999, 2499), s_now));

        var d = LocalRelayHitchDetector.AnalyzeForTests(
            Sample("GET", "/a/seg.ts", "https://h/p/seg.ts?x=1", 503, 1500, 2500, wait: 1000, failure: "timeout"), s_now)!.Value;
        Assert.Equal("segment", d.Kind);
        Assert.Equal("segment-http-503,segment-timeout,slow-upstream-headers,slow-segment-total,server-generation-wait", d.Reasons);
        Assert.Equal("-", d.StreamId);
        Assert.Equal(-1, d.Segment);
        Assert.Equal(-1, d.PreviousSegment);
        Assert.Equal(-1, d.GapMilliseconds);
        Assert.Equal(1500, d.HeaderMilliseconds);
        Assert.Equal(2500, d.TotalMilliseconds);

        d = LocalRelayHitchDetector.AnalyzeForTests(
            Sample("GET", "/a/seg.m4s", "not a url", 200, 10, 2500), s_now)!.Value;
        Assert.Equal("segment", d.Kind);
        Assert.Equal("slow-segment-total", d.Reasons);

        d = LocalRelayHitchDetector.AnalyzeForTests(
            Sample("GET", "/a/m.m3u8", "https://h/p/m.m3u8?q=1", 404, 2000, 2500, failure: "reset"), s_now)!.Value;
        Assert.Equal("manifest", d.Kind);
        Assert.Equal("manifest-http-404,manifest-reset,slow-manifest-headers,slow-manifest-total", d.Reasons);
        Assert.Equal("-", d.StreamId);

        d = LocalRelayHitchDetector.AnalyzeForTests(
            Sample("GET", "/a/index.mpd", "https://h/p/x", 200, 2000, 10), s_now)!.Value;
        Assert.Equal("manifest", d.Kind);
        Assert.Equal("slow-manifest-headers", d.Reasons);

        d = LocalRelayHitchDetector.AnalyzeForTests(Sample("GET", "/a/seg.ts", lazy, 200, 1600, 10), s_now)!.Value;
        Assert.Equal("segment", d.Kind);
        Assert.Equal("slow-upstream-headers", d.Reasons);
        Assert.Equal("wk_s1", d.StreamId);
        Assert.Equal(10, d.Segment);
        Assert.Equal(-1, d.PreviousSegment);
        Assert.Equal(-1, d.GapMilliseconds);

        string skip = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000013.ts";
        d = LocalRelayHitchDetector.AnalyzeForTests(Sample("GET", "/a/seg.ts", skip, 200, 10, 10), s_now.AddSeconds(3))!.Value;
        Assert.Equal("segment-skip", d.Reasons);
        Assert.Equal(10, d.PreviousSegment);
        Assert.Equal(3000, d.GapMilliseconds);
        Assert.Equal(13, d.Segment);

        string back = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000005.ts";
        d = LocalRelayHitchDetector.AnalyzeForTests(Sample("GET", "/a/seg.ts", back, 200, 10, 10), s_now.AddSeconds(4))!.Value;
        Assert.Equal("segment-backtrack", d.Reasons);
        Assert.Equal(13, d.PreviousSegment);
    }

    [Theory]
    [InlineData("https://h/api/proxy/lazy-hls/wk_a/seg_000291.ts", true, "wk_a", 291)]
    [InlineData("https://h/api/proxy/LAZY-HLS/wk_a/x/seg_7.ts?q=1", true, "wk_a", 7)]
    [InlineData("https://h/api/proxy/lazy-hls/wk_a/index.m3u8", false, "", -1)]
    [InlineData("https://h/api/proxy/lazy-hls/seg_1.ts", false, "", -1)]
    [InlineData("relative/lazy-hls/wk_a/seg_1.ts", false, "", -1)]
    public void HitchDetector_LazyParseGolden(string url, bool ok, string stream, int segment)
    {
        Assert.Equal(ok, LocalRelayHitchDetector.TryParseLazyHlsSegment(url, out string s, out int n));
        Assert.Equal(stream, s);
        Assert.Equal(segment, n);
    }
}
