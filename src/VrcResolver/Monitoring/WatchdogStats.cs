using VrcResolver.Shared;

namespace VrcResolver;

// Process-wide counters surfaced by the periodic Heartbeat ticker. All
// counters are interlocked-incremented so increments from the IPC accept
// loop, the WS dispatch loop, and the run loop don't lose updates under
// concurrent resolves. Bytes counter caps at long.MaxValue (~9.2 EB --
// effectively unbounded for any conceivable session).
//
// Stats are session-scoped: reset to zero on each watchdog start. Not
// persisted across launches. No platform-specific APIs -- kept platform-
// agnostic so MeshClient (which isn't [SupportedOSPlatform("windows")])
// can call it without CA1416.
internal static class WatchdogStats
{
    private const int BandwidthBucketCount = 32;

    public static readonly DateTime StartUtc = DateTime.UtcNow;

    private static long _resolvesTotal;
    private static long _resolvesViaLhYt;
    private static long _resolvesCacheHits;
    private static long _bytesEstimateTotal;
    private static long _reconnectCount;
    private static long _relayRequestsTotal;
    private static long _relayBytesTotal;
    private static long _upstreamRelayRequestsTotal;
    private static long _upstreamRelayBytesTotal;
    private static long _lastRelayTicksUtc;
    private static long _lastUpstreamRelayTicksUtc;
    private static readonly object s_bandwidthLock = new();
    private static readonly long[] s_relayBandwidthSeconds = new long[BandwidthBucketCount];
    private static readonly long[] s_relayBandwidthBytes = new long[BandwidthBucketCount];

    public static long ResolvesTotal => Interlocked.Read(ref _resolvesTotal);
    public static long ResolvesViaLhYt => Interlocked.Read(ref _resolvesViaLhYt);
    public static long ResolvesCacheHits => Interlocked.Read(ref _resolvesCacheHits);
    public static long BytesEstimateTotal => Interlocked.Read(ref _bytesEstimateTotal);
    public static long ReconnectCount => Interlocked.Read(ref _reconnectCount);
    public static long RelayRequestsTotal => Interlocked.Read(ref _relayRequestsTotal);
    public static long RelayBytesTotal => Interlocked.Read(ref _relayBytesTotal);
    public static long UpstreamRelayRequestsTotal => Interlocked.Read(ref _upstreamRelayRequestsTotal);
    public static long UpstreamRelayBytesTotal => Interlocked.Read(ref _upstreamRelayBytesTotal);

    public static void RecordResolve(bool viaLhYt)
    {
        Interlocked.Increment(ref _resolvesTotal);
        if (viaLhYt) Interlocked.Increment(ref _resolvesViaLhYt);
    }

    public static void RecordCacheHit()
    {
        Interlocked.Increment(ref _resolvesCacheHits);
    }

    public static void RecordBytesEstimate(long bytes)
    {
        if (bytes <= 0) return;
        Interlocked.Add(ref _bytesEstimateTotal, bytes);
    }

    public static void RecordReconnect()
    {
        Interlocked.Increment(ref _reconnectCount);
    }

    public static void RecordRelayRequest(string targetUrl)
    {
        Interlocked.Increment(ref _relayRequestsTotal);
        if (IsUpstreamTarget(targetUrl))
            Interlocked.Increment(ref _upstreamRelayRequestsTotal);
    }

    public static void RecordRelayBytes(string targetUrl, long bytes)
    {
        RecordRelayBytesAt(targetUrl, bytes, DateTime.UtcNow);
    }

    internal static void RecordRelayBytesAt(string targetUrl, long bytes, DateTime nowUtc)
    {
        if (bytes <= 0) return;
        Interlocked.Add(ref _relayBytesTotal, bytes);
        if (IsUpstreamTarget(targetUrl))
            Interlocked.Add(ref _upstreamRelayBytesTotal, bytes);
        RecordRelayBandwidth(bytes, nowUtc);
        TouchRelay(targetUrl, bytes, nowUtc);
    }

    public static WatchdogActivitySnapshot GetActivitySnapshot()
    {
        return new WatchdogActivitySnapshot(
            ResolvesTotal,
            ResolvesViaLhYt,
            ResolvesCacheHits,
            BytesEstimateTotal,
            ReconnectCount,
            RelayRequestsTotal,
            RelayBytesTotal,
            UpstreamRelayRequestsTotal,
            UpstreamRelayBytesTotal,
            TicksToUtc(Interlocked.Read(ref _lastRelayTicksUtc)),
            TicksToUtc(Interlocked.Read(ref _lastUpstreamRelayTicksUtc)));
    }

    public static WatchdogBandwidthSnapshot GetBandwidthSnapshot()
    {
        return GetBandwidthSnapshot(DateTime.UtcNow, seconds: 24);
    }

    internal static WatchdogBandwidthSnapshot GetBandwidthSnapshot(DateTime nowUtc, int seconds)
    {
        seconds = Math.Clamp(seconds, 1, BandwidthBucketCount);
        long nowSecond = ToUnixSecond(nowUtc);
        var history = new long[seconds];

        lock (s_bandwidthLock)
        {
            for (int i = 0; i < seconds; i++)
            {
                long second = nowSecond - (seconds - 1 - i);
                int index = BucketIndex(second);
                if (s_relayBandwidthSeconds[index] == second)
                    history[i] = s_relayBandwidthBytes[index];
            }
        }

        long current = history.Length == 0 ? 0 : history[^1];
        long peak = 0;
        foreach (long value in history)
            if (value > peak) peak = value;
        return new WatchdogBandwidthSnapshot(current, peak, history);
    }

    internal static void ResetForTests()
    {
        Interlocked.Exchange(ref _resolvesTotal, 0);
        Interlocked.Exchange(ref _resolvesViaLhYt, 0);
        Interlocked.Exchange(ref _resolvesCacheHits, 0);
        Interlocked.Exchange(ref _bytesEstimateTotal, 0);
        Interlocked.Exchange(ref _reconnectCount, 0);
        Interlocked.Exchange(ref _relayRequestsTotal, 0);
        Interlocked.Exchange(ref _relayBytesTotal, 0);
        Interlocked.Exchange(ref _upstreamRelayRequestsTotal, 0);
        Interlocked.Exchange(ref _upstreamRelayBytesTotal, 0);
        Interlocked.Exchange(ref _lastRelayTicksUtc, 0);
        Interlocked.Exchange(ref _lastUpstreamRelayTicksUtc, 0);
        lock (s_bandwidthLock)
        {
            Array.Clear(s_relayBandwidthSeconds);
            Array.Clear(s_relayBandwidthBytes);
        }
    }

    private static void TouchRelay(string targetUrl, long bytes, DateTime nowUtc)
    {
        long nowTicks = nowUtc.Ticks;
        Interlocked.Exchange(ref _lastRelayTicksUtc, nowTicks);
        if (bytes > 0 && IsUpstreamTarget(targetUrl))
            Interlocked.Exchange(ref _lastUpstreamRelayTicksUtc, nowTicks);
    }

    private static void RecordRelayBandwidth(long bytes, DateTime nowUtc)
    {
        long second = ToUnixSecond(nowUtc);
        int index = BucketIndex(second);
        lock (s_bandwidthLock)
        {
            if (s_relayBandwidthSeconds[index] != second)
            {
                s_relayBandwidthSeconds[index] = second;
                s_relayBandwidthBytes[index] = 0;
            }
            s_relayBandwidthBytes[index] = SaturatingAdd(s_relayBandwidthBytes[index], bytes);
        }
    }

    private static long ToUnixSecond(DateTime utc)
    {
        if (utc.Kind != DateTimeKind.Utc)
            utc = utc.ToUniversalTime();
        return utc.Ticks / TimeSpan.TicksPerSecond;
    }

    private static int BucketIndex(long second)
    {
        long mod = second % BandwidthBucketCount;
        if (mod < 0) mod += BandwidthBucketCount;
        return (int)mod;
    }

    private static long SaturatingAdd(long current, long value)
    {
        if (value > 0 && current > long.MaxValue - value)
            return long.MaxValue;
        return current + value;
    }

    private static bool IsUpstreamTarget(string targetUrl)
    {
        return Uri.TryCreate(targetUrl, UriKind.Absolute, out var uri)
            && FirstPartyUrlPolicy.IsFirstPartyHost(uri.Host);
    }

    private static DateTime? TicksToUtc(long ticks)
    {
        return ticks <= 0 ? null : new DateTime(ticks, DateTimeKind.Utc);
    }
}

internal readonly record struct WatchdogBandwidthSnapshot(
    long CurrentBytesPerSecond,
    long PeakBytesPerSecond,
    IReadOnlyList<long> HistoryBytesPerSecond)
{
    public bool HasTraffic => PeakBytesPerSecond > 0;
}

internal readonly record struct WatchdogActivitySnapshot(
    long ResolvesTotal,
    long ResolvesViaLhYt,
    long ResolvesCacheHits,
    long BytesEstimateTotal,
    long ReconnectCount,
    long RelayRequestsTotal,
    long RelayBytesTotal,
    long UpstreamRelayRequestsTotal,
    long UpstreamRelayBytesTotal,
    DateTime? LastRelayUtc,
    DateTime? LastUpstreamRelayUtc)
{
    public bool RelayActive(DateTime nowUtc, TimeSpan window)
    {
        return LastRelayUtc.HasValue && nowUtc - LastRelayUtc.Value <= window;
    }

    public bool UpstreamActive(DateTime nowUtc, TimeSpan window)
    {
        return LastUpstreamRelayUtc.HasValue && nowUtc - LastUpstreamRelayUtc.Value <= window;
    }
}
