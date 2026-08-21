using System.Net;
using System.Net.Sockets;
using System.Runtime.Versioning;
using System.Text;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public class LocalRelayServerTests
{
    [Theory]
    [InlineData("127.0.0.1")]
    [InlineData("localhost")]
    [InlineData("169.254.169.254")]
    [InlineData("10.0.0.5")]
    public async Task GuardedConnect_RefusesBlockedAddresses(string host)
    {
        await Assert.ThrowsAsync<HttpRequestException>(async () =>
            await GuardedRelayConnect.ConnectAsync(new DnsEndPoint(host, 80), CancellationToken.None));
    }

    [Fact]
    public async Task Relay_RejectsRequestsCarryingAnOriginHeader()
    {
        int port;
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();

        var relay = new LocalRelayServer(port);
        relay.Start();
        try
        {
            using var http = new HttpClient();
            string playUrl = "http://127.0.0.1:" + port + "/play/abc/manifest.m3u8?target="
                + LocalRelayTargetResolver.EncodeTargetParam(
                    "https://proxy.whyknot.dev/api/proxy/manifest.m3u8?q=abc");

            // A browser fetch carries Origin; the relay must refuse before
            // any upstream work even though the target itself is allowed.
            using var browserReq = new HttpRequestMessage(HttpMethod.Get, playUrl);
            browserReq.Headers.TryAddWithoutValidation("Origin", "http://example.com");
            using var browserResp = await http.SendAsync(browserReq);
            Assert.Equal(HttpStatusCode.Forbidden, browserResp.StatusCode);

            // Without Origin the pipeline is reachable (404 = path gate,
            // proving the listener is alive and the reject above was the
            // origin check, not a dead server).
            using var mediaResp = await http.GetAsync("http://127.0.0.1:" + port + "/not-play");
            Assert.Equal(HttpStatusCode.NotFound, mediaResp.StatusCode);
        }
        finally
        {
            await relay.StopAsync();
        }
    }

    [Fact]
    public void EncodeTargetParam_RoundTrips()
    {
        string url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=AbC-_dEf=&extra=hi%20there";
        string encoded = LocalRelayTargetResolver.EncodeTargetParam(url);

        Assert.DoesNotContain("+", encoded);
        Assert.DoesNotContain("/", encoded);
        Assert.DoesNotContain("=", encoded);
        Assert.DoesNotContain(" ", encoded);

        string b64 = encoded.Replace('-', '+').Replace('_', '/');
        switch (b64.Length % 4)
        {
            case 2: b64 += "=="; break;
            case 3: b64 += "="; break;
        }

        string decoded = Encoding.UTF8.GetString(Convert.FromBase64String(b64));
        Assert.Equal(url, decoded);
    }

    [Theory]
    [InlineData("/play/7f8a9b0c1d2e/manifest.m3u8", "/play/7f8a9b0c1d2e/")]
    [InlineData("/play/7f8a9b0c1d2e/sub/manifest.m3u8", "/play/7f8a9b0c1d2e/sub/")]
    [InlineData("/play/manifest.m3u8", "/play/")]
    [InlineData("/play", "/play/")]
    [InlineData("/play/", "/play/")]
    public void LocalPrefixForPath_ReturnsDirectoryPrefix(string path, string expected)
    {
        Assert.Equal(expected, LocalRelayTargetResolver.LocalPrefixForPath(path));
    }

    [Theory]
    [InlineData("")]
    [InlineData("/")]
    [InlineData("/playlist/manifest.m3u8")]
    [InlineData("/playback/manifest.m3u8")]
    public void LocalPrefixForPath_NonPlayPaths_FallBackToPlayRoot(string path)
    {
        Assert.Equal("/play/", LocalRelayTargetResolver.LocalPrefixForPath(path));
    }

    [Fact]
    public void TryResolveRelativeTarget_MapsNamespacedSegmentToUpstreamBase()
    {
        bool ok = LocalRelayTargetResolver.TryResolveRelativeTarget(
            "/play/7f8a9b0c1d2e/seg_000001.ts",
            "?range=bytes%3D0-",
            "/play/7f8a9b0c1d2e/",
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/",
            out string targetUrl);

        Assert.True(ok);
        Assert.Equal(
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?range=bytes%3D0-",
            targetUrl);
    }

    [Fact]
    public void TryResolveRelativeTarget_UsesDirectoryOfManifestUrl()
    {
        string manifestUrl = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8?token=abc";
        string upstreamBase = new Uri(new Uri(manifestUrl), ".").ToString();

        bool ok = LocalRelayTargetResolver.TryResolveRelativeTarget(
            "/play/7f8a9b0c1d2e/seg_000002.ts",
            "",
            "/play/7f8a9b0c1d2e/",
            upstreamBase,
            out string targetUrl);

        Assert.True(ok);
        Assert.Equal(
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000002.ts",
            targetUrl);
    }

    [Fact]
    public void TryResolveRelativeTarget_RejectsPathOutsideNamespace()
    {
        bool ok = LocalRelayTargetResolver.TryResolveRelativeTarget(
            "/play/other/seg_000001.ts",
            "",
            "/play/7f8a9b0c1d2e/",
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/",
            out string targetUrl);

        Assert.False(ok);
        Assert.Equal("", targetUrl);
    }

    [Fact]
    public void TryResolveRelativeTarget_RejectsNamespaceRootFetch()
    {
        bool ok = LocalRelayTargetResolver.TryResolveRelativeTarget(
            "/play/7f8a9b0c1d2e/",
            "",
            "/play/7f8a9b0c1d2e/",
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/",
            out string targetUrl);

        Assert.False(ok);
        Assert.Equal("", targetUrl);
    }

    [Fact]
    public void TryResolve_TargetParamRegistersRelativeNamespace()
    {
        var resolver = new LocalRelayTargetResolver();
        string manifestUrl = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8";
        string targetParam = LocalRelayTargetResolver.EncodeTargetParam(manifestUrl);

        bool manifestOk = resolver.TryResolve(
            "/play/7f8a9b0c1d2e/manifest.m3u8",
            "?target=" + targetParam,
            targetParam,
            out LocalRelayTarget manifestTarget);

        bool segmentOk = resolver.TryResolve(
            "/play/7f8a9b0c1d2e/seg_000003.ts",
            "",
            null,
            out LocalRelayTarget segmentTarget);

        Assert.True(manifestOk);
        Assert.Equal("target", manifestTarget.Kind);
        Assert.Equal(manifestUrl, manifestTarget.Url);

        Assert.True(segmentOk);
        Assert.Equal("relative", segmentTarget.Kind);
        Assert.Equal(
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000003.ts",
            segmentTarget.Url);
    }

    [Theory]
    [InlineData("localhost.youtube.com:51234", 51234, true)]
    [InlineData("LOCALHOST.YOUTUBE.COM:51234", 51234, true)]
    [InlineData("127.0.0.1:51234", 51234, true)]
    [InlineData("localhost:51234", 51234, true)]
    [InlineData("us1.vrcresolver.com:51234", 51234, false)]
    [InlineData("localhost.youtube.com:51235", 51234, false)]
    [InlineData("", 51234, false)]
    public void Security_AllowsOnlyExpectedLocalHosts(string host, int port, bool expected)
    {
        Assert.Equal(expected, LocalRelaySecurity.IsAllowedHostHeader(host, port));
    }

    [Theory]
    [InlineData("https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8", true)]
    [InlineData("https://us1.vrcresolver.com/api/popcorn/proxy/manifest.m3u8?clientId=c&index=1", true)]
    // The server still emits playback URLs under the pre-rename domain for
    // wire compatibility; the relay must keep accepting that family.
    [InlineData("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true)]
    [InlineData("https://whyknot.dev/api/proxy/lazy-hls/wk_abc/index.m3u8", true)]
    [InlineData("https://us1.vrcresolver.com/api/restream/shared-status", false)]
    [InlineData("https://example.com/api/proxy/manifest.m3u8?q=abc", false)]
    [InlineData("file:///C:/Windows/win.ini", false)]
    [InlineData("not a url", false)]
    public void Security_AllowsOnlyFirstPartyProxyTargets(string targetUrl, bool expected)
    {
        Assert.Equal(expected, LocalRelaySecurity.IsAllowedTargetUrl(targetUrl, out _));
    }

    [Fact]
    public void ManifestLocalizer_RewritesFirstPartyPopcornProxyUrls()
    {
        string upstream = "https://eu1.vrcresolver.com/api/popcorn/proxy/seg.ts?clientId=c&url=abc";
        string manifest = "#EXTM3U\n#EXTINF:4.000,\n" + upstream + "\n";

        string localized = LocalRelayManifestLocalizer.Localize(
            manifest,
            "/play/7f8a9b0c1d2e/manifest.m3u8");

        Assert.DoesNotContain(upstream, localized);
        Assert.Contains("proxy/", localized);
        Assert.Contains("/seg.ts?target=", localized);
    }

    [Fact]
    public void ManifestLocalizer_RewritesFirstPartyProxyLinesToLocalTargets()
    {
        string upstream = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8";
        string manifest = "#EXTM3U\n"
            + "#EXT-X-STREAM-INF:BANDWIDTH=1000000\n"
            + upstream + "\n";

        string localized = LocalRelayManifestLocalizer.Localize(
            manifest,
            "/play/7f8a9b0c1d2e/manifest.m3u8");

        Assert.DoesNotContain("https://us1.vrcresolver.com", localized);
        Assert.Contains("/index.m3u8?target=", localized);
        Assert.Contains("proxy/", localized);

        string targetParam = ExtractTargetParam(localized);
        var resolver = new LocalRelayTargetResolver();
        bool ok = resolver.TryResolve(
            "/play/7f8a9b0c1d2e/" + ExtractLocalPath(localized),
            "?target=" + targetParam,
            targetParam,
            out LocalRelayTarget target);

        Assert.True(ok);
        Assert.Equal(upstream, target.Url);
    }

    [Fact]
    public void ManifestLocalizer_RewritesFirstPartyQuotedUris()
    {
        string upstream = "https://eu1.vrcresolver.com/api/proxy/seg.ts?url=abc";
        string manifest = "#EXTM3U\n"
            + "#EXT-X-KEY:METHOD=AES-128,URI=\"" + upstream + "\"\n"
            + "#EXTINF:4.000,\n"
            + "seg_000001.ts\n";

        string localized = LocalRelayManifestLocalizer.Localize(
            manifest,
            "/play/7f8a9b0c1d2e/proxy/index.m3u8");

        Assert.DoesNotContain(upstream, localized);
        Assert.Contains("URI=\"proxy/", localized);
        Assert.Contains("/seg.ts?target=", localized);
        Assert.Contains("seg_000001.ts", localized);
    }

    [Fact]
    public void ManifestLocalizer_GivesSameFileNameTargetsDistinctLocalNamespaces()
    {
        string first = "https://us1.vrcresolver.com/api/proxy/a/index.m3u8?x=1";
        string second = "https://us1.vrcresolver.com/api/proxy/b/index.m3u8?x=2";
        string manifest = "#EXTM3U\n"
            + "#EXT-X-STREAM-INF:BANDWIDTH=1000000\n"
            + first + "\n"
            + "#EXT-X-STREAM-INF:BANDWIDTH=2000000\n"
            + second + "\n";

        string localized = LocalRelayManifestLocalizer.Localize(
            manifest,
            "/play/7f8a9b0c1d2e/manifest.m3u8");

        string[] localTargets = localized.Split('\n')
            .Where(line => line.StartsWith("proxy/", StringComparison.Ordinal))
            .ToArray();

        Assert.Equal(2, localTargets.Length);
        Assert.NotEqual(
            localTargets[0].Substring(0, localTargets[0].IndexOf("/index.m3u8", StringComparison.Ordinal)),
            localTargets[1].Substring(0, localTargets[1].IndexOf("/index.m3u8", StringComparison.Ordinal)));
    }

    [Fact]
    public void ManifestLocalizer_RewritesFirstPartyUrlsInsideXmlText()
    {
        string upstream = "https://eu1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.m4s";
        string manifest = "<MPD><Period><BaseURL>" + upstream + "</BaseURL></Period></MPD>";

        string localized = LocalRelayManifestLocalizer.Localize(
            manifest,
            "/play/7f8a9b0c1d2e/manifest.mpd");

        Assert.DoesNotContain(upstream, localized);
        Assert.Contains("<BaseURL>proxy/", localized);
        Assert.Contains("/seg_000001.m4s?target=", localized);
    }

    [Fact]
    public void ManifestLocalizer_LeavesThirdPartyAbsoluteUrlsUntouched()
    {
        string upstream = "https://cdn.example.com/video/seg.ts";
        string manifest = "#EXTM3U\n#EXTINF:4.000,\n" + upstream;

        string localized = LocalRelayManifestLocalizer.Localize(
            manifest,
            "/play/7f8a9b0c1d2e/manifest.m3u8");

        Assert.Equal(manifest, localized);
    }

    [Fact]
    public async Task ManifestLocalizer_StreamsLargePlaybackTokenManifest()
    {
        const int segmentCount = 5000;
        var sb = new StringBuilder();
        sb.Append("#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n");
        string token = new string('A', 900);
        for (int i = 0; i < segmentCount; i++)
        {
            sb.Append("#EXTINF:2.000,\n");
            sb.Append("https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_");
            sb.Append(i.ToString("D6"));
            sb.Append(".ts?playback_id=");
            sb.Append(token);
            sb.Append('\n');
        }
        sb.Append("#EXT-X-ENDLIST\n");
        byte[] inputBytes = Encoding.UTF8.GetBytes(sb.ToString());

        Assert.True(inputBytes.Length > 4 * 1024 * 1024,
            "synthetic manifest must exceed the legacy 4 MiB cap to exercise the streaming path");

        using var input = new MemoryStream(inputBytes, writable: false);
        using var output = new MemoryStream();
        var result = await LocalRelayManifestLocalizer.LocalizeStreamAsync(
            input,
            inputEncoding: null,
            output,
            LocalRelayManifestLocalizer.MaxManifestBytes,
            CancellationToken.None);

        Assert.False(result.Exceeded);
        Assert.True(result.Changed);

        output.Position = 0;
        string localized = new StreamReader(output, Encoding.UTF8).ReadToEnd();

        Assert.DoesNotContain("https://us1.vrcresolver.com", localized);
        int proxyLines = 0;
        foreach (string line in localized.Split('\n'))
        {
            if (line.StartsWith("proxy/", StringComparison.Ordinal)) proxyLines++;
        }
        Assert.Equal(segmentCount, proxyLines);
        Assert.Contains("#EXT-X-ENDLIST", localized);
    }

    [Fact]
    public async Task ManifestLocalizer_StreamAndStringProduceSameOutput()
    {
        string manifest = "#EXTM3U\n"
            + "#EXT-X-STREAM-INF:BANDWIDTH=1000000\n"
            + "https://us1.vrcresolver.com/api/proxy/a/index.m3u8?x=1\n"
            + "#EXTINF:2,\n"
            + "https://eu1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=tok\n"
            + "#EXT-X-KEY:METHOD=AES-128,URI=\"https://eu1.vrcresolver.com/api/proxy/key.bin?clientId=c\"\n"
            + "https://cdn.example.com/video/seg.ts\n";

        string stringResult = LocalRelayManifestLocalizer.Localize(manifest, "/play/abc/manifest.m3u8");

        using var input = new MemoryStream(Encoding.UTF8.GetBytes(manifest));
        using var output = new MemoryStream();
        await LocalRelayManifestLocalizer.LocalizeStreamAsync(
            input,
            inputEncoding: null,
            output,
            LocalRelayManifestLocalizer.MaxManifestBytes,
            CancellationToken.None);
        output.Position = 0;
        string streamResult = new StreamReader(output, Encoding.UTF8).ReadToEnd();

        Assert.Equal(stringResult, streamResult);
    }

    [Theory]
    // Plain m3u8 / mpd extensions always classify as manifests.
    [InlineData("/play/a/manifest.m3u8", "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc", "text/plain", true)]
    [InlineData("/play/a/index.m3u8", "https://us1.vrcresolver.com/api/proxy/index.m3u8", "application/vnd.apple.mpegurl", true)]
    [InlineData("/play/a/manifest.mpd", "https://us1.vrcresolver.com/api/proxy/manifest.mpd", "application/dash+xml", true)]
    // Query strings don't break the extension probe.
    [InlineData("/play/a/manifest.m3u8?token=abc", "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?token=abc", "application/vnd.apple.mpegurl", true)]
    // Local path has a non-manifest extension; targetUrl carries .m3u8 -- second branch catches it.
    [InlineData("/play/a/manifest.bin", "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc", "application/octet-stream", true)]
    // Bare "manifest" (no extension) is treated as a manifest -- some providers ship that.
    [InlineData("/play/a/manifest", "https://us1.vrcresolver.com/api/proxy/manifest", "application/vnd.apple.mpegurl", true)]
    // /manifest.mp4 was the 2026-05-22 YouTube load_failure bug: the
    // relay's own progressive-MP4 URL pattern was treated as an HLS
    // manifest, run through the line-by-line text rewriter, and shipped
    // as Transfer-Encoding: chunked with no Content-Length, so WMF/NSPlayer
    // disconnected on the first byte.
    [InlineData("/play/a/manifest.mp4", "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=abc", "video/mp4", false)]
    [InlineData("/play/a/MANIFEST.MP4", "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=abc", "video/mp4", false)]
    // Segment URLs never qualify as manifests, regardless of "manifest" appearing in path.
    [InlineData("/play/a/seg.ts", "https://us1.vrcresolver.com/api/proxy/seg.ts?url=abc", "video/mp2t", false)]
    [InlineData("/play/a/manifest_archive/seg.ts", "https://us1.vrcresolver.com/api/proxy/seg.ts", "video/mp2t", false)]
    // Content-type fallback: a .ts URL that's actually an m3u8 (Tubi pattern) still classifies.
    [InlineData("/play/a/foo.ts", "https://example.test/playlist.ts", "application/vnd.apple.mpegurl", true)]
    public void ManifestLocalizer_DetectsOnlyManifestShapes(
        string localPath,
        string targetUrl,
        string mediaType,
        bool expected)
    {
        var contentType = new System.Net.Http.Headers.MediaTypeHeaderValue(mediaType);
        Assert.Equal(expected, LocalRelayManifestLocalizer.IsLikelyManifest(localPath, targetUrl, contentType));
    }

    [Fact]
    public void HitchDetector_ParsesLazyHlsSegments()
    {
        bool ok = LocalRelayHitchDetector.TryParseLazyHlsSegment(
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000291.ts",
            out string streamId,
            out int segment);

        Assert.True(ok);
        Assert.Equal("wk_abc", streamId);
        Assert.Equal(291, segment);
    }

    [Fact]
    public void HitchDetector_FlagsSlowSegmentResponses()
    {
        LocalRelayHitchDetector.ResetForTests();

        LocalRelayHitchDiagnostic? diagnostic = LocalRelayHitchDetector.AnalyzeForTests(
            new LocalRelayTimingSample(
                "GET",
                "/play/a/seg_000001.ts",
                "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts",
                206,
                HeaderMilliseconds: 1800,
                TotalMilliseconds: 3200,
                BytesOut: 128,
                LazyHlsState: "HIT",
                LazyHlsWaitMilliseconds: -1,
                LazyHlsGenerator: null,
                Failure: null),
            new DateTime(2026, 5, 10, 23, 0, 0, DateTimeKind.Utc));

        Assert.NotNull(diagnostic);
        Assert.Contains("slow-upstream-headers", diagnostic!.Value.Reasons);
        Assert.Contains("slow-segment-total", diagnostic.Value.Reasons);
    }

    [Fact]
    public void HitchDetector_FlagsSegmentRetries()
    {
        LocalRelayHitchDetector.ResetForTests();
        var now = new DateTime(2026, 5, 10, 23, 0, 0, DateTimeKind.Utc);
        var sample = new LocalRelayTimingSample(
            "GET",
            "/play/a/seg_000042.ts",
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000042.ts",
            206,
            HeaderMilliseconds: 20,
            TotalMilliseconds: 40,
            BytesOut: 128,
            LazyHlsState: "HIT",
            LazyHlsWaitMilliseconds: -1,
            LazyHlsGenerator: null,
            Failure: null);

        Assert.Null(LocalRelayHitchDetector.AnalyzeForTests(sample, now));
        LocalRelayHitchDiagnostic? retry = LocalRelayHitchDetector.AnalyzeForTests(
            sample,
            now.AddSeconds(2));

        Assert.NotNull(retry);
        Assert.Contains("segment-retry", retry!.Value.Reasons);
        Assert.Equal(42, retry.Value.PreviousSegment);
    }

    private static string ExtractTargetParam(string localized)
    {
        const string marker = "?target=";
        int start = localized.IndexOf(marker, StringComparison.Ordinal);
        Assert.True(start >= 0, "localized manifest did not contain target param");
        start += marker.Length;
        int end = localized.IndexOfAny(new[] { '\r', '\n', '"' }, start);
        if (end < 0) end = localized.Length;
        return localized.Substring(start, end - start);
    }

    private static string ExtractLocalPath(string localized)
    {
        string[] lines = localized.Split('\n');
        string? local = lines.FirstOrDefault(line => line.StartsWith("proxy/", StringComparison.Ordinal));
        Assert.False(string.IsNullOrWhiteSpace(local), "localized manifest did not contain a local proxy path");
        int query = local.IndexOf('?');
        Assert.True(query > 0, "localized proxy path did not contain a query");
        return local.Substring(0, query);
    }
}
