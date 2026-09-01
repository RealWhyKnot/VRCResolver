namespace VrcResolver.Shared;

public static class ResolveBudget
{
    public static readonly TimeSpan Total = TimeSpan.FromSeconds(28);

    public static readonly TimeSpan OgReserve = TimeSpan.FromSeconds(5);

    public static readonly TimeSpan ReAskReserve = TimeSpan.FromSeconds(5);

    public static readonly TimeSpan OgHardCap = TimeSpan.FromMinutes(5);

    public const int ReAskMinWindowMs = 2250;

    public const int NoHintMinRemainingMs = 3000;

    public static bool ReAskFits(long? retryAtElapsedMs)
        => retryAtElapsedMs is not long retryAt
            || (long)Total.TotalMilliseconds - retryAt >= ReAskMinWindowMs;

    public static long? ReAskDelayMs(long elapsedMs, long? retryAtElapsedMs)
    {
        long totalMs = (long)Total.TotalMilliseconds;
        if (retryAtElapsedMs is not long retryAt)
            return totalMs - elapsedMs >= NoHintMinRemainingMs ? 0 : null;
        long startMs = Math.Max(retryAt, elapsedMs);
        return totalMs - startMs >= ReAskMinWindowMs ? startMs - elapsedMs : null;
    }

    public static TimeSpan OgWindow(TimeSpan elapsed, bool reAskAvailable, long? retryAtElapsedMs = null)
    {
        if (!reAskAvailable) return OgHardCap;
        var remaining = Total - elapsed - (ReAskFits(retryAtElapsedMs) ? ReAskReserve : TimeSpan.Zero);
        return remaining < OgReserve ? OgReserve : remaining;
    }
}
