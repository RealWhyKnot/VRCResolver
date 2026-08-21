using System.Collections.Concurrent;
using System.Globalization;
using VrcResolver.Shared;

namespace VrcResolver;

internal readonly record struct LocalRelayTimingSample(
    string Method,
    string LocalPath,
    string TargetUrl,
    int StatusCode,
    long HeaderMilliseconds,
    long TotalMilliseconds,
    long BytesOut,
    string? LazyHlsState,
    long LazyHlsWaitMilliseconds,
    string? LazyHlsGenerator,
    string? Failure);

internal readonly record struct LocalRelayHitchDiagnostic(
    string Kind,
    string Reasons,
    string StreamId,
    int Segment,
    int PreviousSegment,
    long GapMilliseconds,
    long HeaderMilliseconds,
    long TotalMilliseconds);

internal static class LocalRelayHitchDetector
{
    private const long SegmentSlowHeaderMs = 1500;
    private const long SegmentSlowTotalMs = 2500;
    private const long ManifestSlowHeaderMs = 2000;
    private const long ManifestSlowTotalMs = 2500;
    private const long ServerGenerationWaitMs = 1000;
    private const long RetryWindowMs = 15000;
    private const long LateNextSegmentRequestMs = 6500;
    private const int MaxTrackedStreams = 256;

    private static readonly ConcurrentDictionary<string, StreamState> s_streams = new();
    private static readonly ConcurrentQueue<string> s_streamOrder = new();

    public static void Record(LocalRelayTimingSample sample)
    {
        LocalRelayHitchDiagnostic? diagnostic = Analyze(sample, DateTime.UtcNow);
        if (!diagnostic.HasValue)
            return;

        LocalRelayHitchDiagnostic d = diagnostic.Value;
        string line = "[relay][hitch] reasons=" + d.Reasons
            + " kind=" + d.Kind
            + " stream=" + Safe(d.StreamId, 64)
            + SegmentPart(d.Segment)
            + PreviousPart(d.PreviousSegment, d.GapMilliseconds)
            + " status=" + sample.StatusCode.ToString(CultureInfo.InvariantCulture)
            + " lazy=" + Safe(sample.LazyHlsState ?? "unknown", 32)
            + WaitPart(sample.LazyHlsWaitMilliseconds)
            + GeneratorPart(sample.LazyHlsGenerator)
            + " header_ms=" + d.HeaderMilliseconds.ToString(CultureInfo.InvariantCulture)
            + " total_ms=" + d.TotalMilliseconds.ToString(CultureInfo.InvariantCulture)
            + " bytes=" + sample.BytesOut.ToString(CultureInfo.InvariantCulture)
            + FailurePart(sample.Failure)
            + " target=" + DescribeUrl(sample.TargetUrl);
        Logger.WarnDiagnostic(
            LogComponent.Relay,
            line,
            "hitch reasons=" + d.Reasons
                + " kind=" + d.Kind
                + SegmentPart(d.Segment)
                + " status=" + sample.StatusCode.ToString(CultureInfo.InvariantCulture)
                + WaitPart(sample.LazyHlsWaitMilliseconds)
                + GeneratorPart(sample.LazyHlsGenerator)
                + " total_ms=" + d.TotalMilliseconds.ToString(CultureInfo.InvariantCulture)
                + FailurePart(sample.Failure));
    }

    internal static LocalRelayHitchDiagnostic? AnalyzeForTests(LocalRelayTimingSample sample, DateTime nowUtc)
        => Analyze(sample, nowUtc);

    internal static void ResetForTests()
    {
        s_streams.Clear();
        while (s_streamOrder.TryDequeue(out _)) { }
    }

    internal static bool TryParseLazyHlsSegment(string url, out string streamId, out int segment)
    {
        streamId = "";
        segment = -1;
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri))
            return false;

        string[] parts = uri.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
        for (int i = 0; i < parts.Length - 2; i++)
        {
            if (!string.Equals(parts[i], "lazy-hls", StringComparison.OrdinalIgnoreCase))
                continue;

            string candidateStream = parts[i + 1];
            for (int j = i + 2; j < parts.Length; j++)
            {
                if (TryParseSegmentFile(parts[j], out int parsed))
                {
                    streamId = candidateStream;
                    segment = parsed;
                    return true;
                }
            }
        }

        return false;
    }

    private static LocalRelayHitchDiagnostic? Analyze(LocalRelayTimingSample sample, DateTime nowUtc)
    {
        if (!string.Equals(sample.Method, "GET", StringComparison.OrdinalIgnoreCase))
            return null;

        var reasons = new List<string>(4);
        bool isLazySegment = TryParseLazyHlsSegment(sample.TargetUrl, out string streamId, out int segment);
        bool isSegment = isLazySegment || IsLikelySegment(sample.LocalPath, sample.TargetUrl);
        bool isManifest = !isSegment && IsLikelyManifest(sample.LocalPath, sample.TargetUrl);

        int previousSegment = -1;
        long gapMs = -1;
        if (isSegment)
        {
            if (sample.StatusCode >= 400)
                reasons.Add("segment-http-" + sample.StatusCode.ToString(CultureInfo.InvariantCulture));
            if (!string.IsNullOrWhiteSpace(sample.Failure))
                reasons.Add("segment-" + sample.Failure);
            if (sample.HeaderMilliseconds >= SegmentSlowHeaderMs)
                reasons.Add("slow-upstream-headers");
            if (sample.TotalMilliseconds >= SegmentSlowTotalMs)
                reasons.Add("slow-segment-total");
            if (sample.LazyHlsWaitMilliseconds >= ServerGenerationWaitMs)
                reasons.Add("server-generation-wait");

            if (isLazySegment)
                AddSequenceReasons(streamId, segment, nowUtc, reasons, out previousSegment, out gapMs);
        }
        else if (isManifest)
        {
            if (sample.StatusCode >= 400)
                reasons.Add("manifest-http-" + sample.StatusCode.ToString(CultureInfo.InvariantCulture));
            if (!string.IsNullOrWhiteSpace(sample.Failure))
                reasons.Add("manifest-" + sample.Failure);
            if (sample.HeaderMilliseconds >= ManifestSlowHeaderMs)
                reasons.Add("slow-manifest-headers");
            if (sample.TotalMilliseconds >= ManifestSlowTotalMs)
                reasons.Add("slow-manifest-total");
        }

        if (reasons.Count == 0)
            return null;

        return new LocalRelayHitchDiagnostic(
            isSegment ? "segment" : "manifest",
            string.Join(",", reasons),
            isLazySegment ? streamId : "-",
            isLazySegment ? segment : -1,
            previousSegment,
            gapMs,
            sample.HeaderMilliseconds,
            sample.TotalMilliseconds);
    }

    private static void AddSequenceReasons(
        string streamId,
        int segment,
        DateTime nowUtc,
        List<string> reasons,
        out int previousSegment,
        out long gapMs)
    {
        previousSegment = -1;
        gapMs = -1;
        if (string.IsNullOrWhiteSpace(streamId) || segment < 0)
            return;

        StreamState state = s_streams.GetOrAdd(streamId, key =>
        {
            s_streamOrder.Enqueue(key);
            TrimStreamTable();
            return new StreamState();
        });

        lock (state)
        {
            previousSegment = state.LastSegment;
            if (state.LastSeenUtc != default)
                gapMs = Math.Max(0, (long)(nowUtc - state.LastSeenUtc).TotalMilliseconds);

            if (state.LastSegment == segment && gapMs >= 0 && gapMs <= RetryWindowMs)
                reasons.Add("segment-retry");
            else if (state.LastSegment >= 0 && segment > state.LastSegment + 1)
                reasons.Add("segment-skip");
            else if (state.LastSegment >= 0 && segment < state.LastSegment)
                reasons.Add("segment-backtrack");
            else if (state.LastSegment >= 0 && segment == state.LastSegment + 1
                && gapMs > LateNextSegmentRequestMs)
                reasons.Add("late-next-segment-request");

            state.LastSegment = segment;
            state.LastSeenUtc = nowUtc;
        }
    }

    private static void TrimStreamTable()
    {
        while (s_streams.Count > MaxTrackedStreams && s_streamOrder.TryDequeue(out string? oldKey))
            s_streams.TryRemove(oldKey, out _);
    }

    private static bool IsLikelySegment(string localPath, string targetUrl)
    {
        string path = PathFor(targetUrl);
        return EndsWithAny(path, ".ts", ".m4s", ".mp4")
            || EndsWithAny(localPath, ".ts", ".m4s", ".mp4");
    }

    private static bool IsLikelyManifest(string localPath, string targetUrl)
    {
        string path = PathFor(targetUrl);
        return EndsWithAny(path, ".m3u8", ".mpd")
            || EndsWithAny(localPath, ".m3u8", ".mpd");
    }

    private static string PathFor(string url)
    {
        return Uri.TryCreate(url, UriKind.Absolute, out var uri) ? uri.AbsolutePath : url;
    }

    private static bool EndsWithAny(string value, params string[] suffixes)
    {
        foreach (string suffix in suffixes)
            if ((value ?? "").EndsWith(suffix, StringComparison.OrdinalIgnoreCase))
                return true;
        return false;
    }

    private static bool TryParseSegmentFile(string name, out int segment)
    {
        segment = -1;
        if (!name.StartsWith("seg_", StringComparison.OrdinalIgnoreCase))
            return false;

        int start = "seg_".Length;
        int end = start;
        while (end < name.Length && name[end] >= '0' && name[end] <= '9')
            end++;
        return end > start
            && int.TryParse(name.AsSpan(start, end - start), NumberStyles.None, CultureInfo.InvariantCulture, out segment);
    }

    private static string DescribeUrl(string url)
    {
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri))
            return Safe(url, 120);
        return Safe(uri.Host + uri.AbsolutePath, 140);
    }

    private static string SegmentPart(int segment)
        => segment >= 0 ? " segment=" + segment.ToString(CultureInfo.InvariantCulture) : "";

    private static string PreviousPart(int previousSegment, long gapMs)
    {
        if (previousSegment < 0)
            return "";
        return " previous_segment=" + previousSegment.ToString(CultureInfo.InvariantCulture)
            + " gap_ms=" + gapMs.ToString(CultureInfo.InvariantCulture);
    }

    private static string WaitPart(long waitMs)
        => waitMs >= 0 ? " lazy_wait_ms=" + waitMs.ToString(CultureInfo.InvariantCulture) : "";

    private static string GeneratorPart(string? generator)
        => string.IsNullOrWhiteSpace(generator) ? "" : " lazy_generator=" + Safe(generator, 32);

    private static string FailurePart(string? failure)
        => string.IsNullOrWhiteSpace(failure) ? "" : " failure=" + Safe(failure, 64);

    private static string Safe(string value, int max)
        => LogUtil.SanitizeForConsole(value ?? "", max);

    private sealed class StreamState
    {
        public int LastSegment = -1;
        public DateTime LastSeenUtc;
    }
}
