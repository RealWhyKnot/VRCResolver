namespace VrcResolver.Shared;

public static class ResolveRetryPolicy
{
    private static readonly string[] RetryableReasons =
    {
        WireConstants.FallbackDiscoveryInProgress,
        WireConstants.FallbackServerUnreachable,
    };

    public const int MaxRetries = 2;

    public const int MinBudgetForRetryMs = 2000;

    public static bool ShouldRetry(string? reason, int attemptsSoFar, long remainingBudgetMs)
    {
        if (attemptsSoFar >= MaxRetries) return false;
        if (remainingBudgetMs < MinBudgetForRetryMs) return false;
        if (reason == null) return false;
        foreach (var r in RetryableReasons)
            if (reason == r) return true;
        return false;
    }

    public static int NextDelayMs(int attempt) => attempt switch
    {
        0 => 750,
        1 => 2250,
        _ => 2250,
    };

    public static int? RetryDelayMs(string? reason, int? retryAfterMs, int attempt, long remainingBudgetMs)
    {
        if (reason == WireConstants.FallbackDiscoveryInProgress && retryAfterMs is int hint)
            return hint <= Math.Min(5000, remainingBudgetMs - MinBudgetForRetryMs) ? hint : null;
        return NextDelayMs(attempt);
    }
}
