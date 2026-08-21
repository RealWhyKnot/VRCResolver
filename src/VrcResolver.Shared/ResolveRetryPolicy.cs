namespace VrcResolver.Shared;

// Pure-static retry policy for the wrapper's resolve loop.
// Extracted so the delay schedule and eligibility rules are testable
// without a running pipe or watchdog process.
public static class ResolveRetryPolicy
{
    // Reasons that are worth retrying: the server hasn't settled yet
    // (discovery_in_progress) or was briefly unreachable (server_unreachable).
    // Structural failures (domain blocked, no matching config, etc.) will not
    // change between attempts so retrying them only wastes budget.
    private static readonly string[] RetryableReasons =
    {
        WireConstants.FallbackDiscoveryInProgress,
        WireConstants.FallbackServerUnreachable,
    };

    // Maximum extra attempts AFTER the first send (so total sends = MaxRetries + 1).
    public const int MaxRetries = 2;

    // Minimum remaining budget required to permit a retry.
    // A retry that would fire within 2s of the deadline is more likely to
    // fail or produce a useless late response than to succeed cleanly.
    public const int MinBudgetForRetryMs = 2000;

    // Returns true when a retry attempt is appropriate.
    //   reason         -- the fallback_native reason string from the server
    //   attemptsSoFar  -- number of retries already sent (0 = no retries yet)
    //   remainingBudgetMs -- ms remaining in the overall wrapper budget
    public static bool ShouldRetry(string? reason, int attemptsSoFar, long remainingBudgetMs)
    {
        if (attemptsSoFar >= MaxRetries) return false;
        if (remainingBudgetMs < MinBudgetForRetryMs) return false;
        if (reason == null) return false;
        foreach (var r in RetryableReasons)
            if (reason == r) return true;
        return false;
    }

    // Returns the delay in milliseconds before sending attempt number `attempt`
    // (0-indexed; attempt=0 -> first retry delay, attempt=1 -> second retry delay).
    // Schedule: 750 ms then 2250 ms -- gives two passes within ~3s total,
    // which fits inside the 28s wrapper budget without starving the og fallback.
    public static int NextDelayMs(int attempt) => attempt switch
    {
        0 => 750,
        1 => 2250,
        _ => 2250,
    };
}
