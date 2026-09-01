using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class ResolveRetryPolicyTests
{
    [Fact]
    public void ShouldRetry_discovery_in_progress_first_attempt_ample_budget_returns_true()
    {
        Assert.True(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackDiscoveryInProgress, attemptsSoFar: 0, remainingBudgetMs: 15000));
    }

    [Fact]
    public void ShouldRetry_discovery_in_progress_max_retries_reached_returns_false()
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackDiscoveryInProgress, attemptsSoFar: ResolveRetryPolicy.MaxRetries, remainingBudgetMs: 15000));
    }

    [Fact]
    public void ShouldRetry_discovery_in_progress_budget_too_small_returns_false()
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackDiscoveryInProgress, attemptsSoFar: 0, remainingBudgetMs: 1500));
    }

    [Fact]
    public void ShouldRetry_non_retryable_reason_returns_false()
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackAllConfigsFailed, attemptsSoFar: 0, remainingBudgetMs: 15000));
    }

    [Fact]
    public void ShouldRetry_server_unreachable_first_attempt_ample_budget_returns_true()
    {
        Assert.True(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackServerUnreachable, attemptsSoFar: 0, remainingBudgetMs: 15000));
    }

    [Fact]
    public void ShouldRetry_client_deadline_exceeded_returns_false()
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackClientDeadlineExceeded, attemptsSoFar: 0, remainingBudgetMs: 15000));
    }

    [Fact]
    public void ShouldRetry_null_reason_returns_false()
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(null, attemptsSoFar: 0, remainingBudgetMs: 15000));
    }

    [Fact]
    public void ShouldRetry_budget_exactly_at_minimum_returns_true()
    {
        Assert.True(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackDiscoveryInProgress, attemptsSoFar: 0,
            remainingBudgetMs: ResolveRetryPolicy.MinBudgetForRetryMs));
    }

    [Fact]
    public void ShouldRetry_budget_one_below_minimum_returns_false()
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(
            WireConstants.FallbackDiscoveryInProgress, attemptsSoFar: 0,
            remainingBudgetMs: ResolveRetryPolicy.MinBudgetForRetryMs - 1));
    }

    [Fact]
    public void NextDelayMs_attempt_zero_returns_750()
    {
        Assert.Equal(750, ResolveRetryPolicy.NextDelayMs(0));
    }

    [Fact]
    public void NextDelayMs_attempt_one_returns_2250()
    {
        Assert.Equal(2250, ResolveRetryPolicy.NextDelayMs(1));
    }

    [Fact]
    public void MaxRetries_is_two()
    {
        Assert.Equal(2, ResolveRetryPolicy.MaxRetries);
    }

    [Fact]
    public void MinBudgetForRetryMs_is_two_seconds()
    {
        Assert.Equal(2000, ResolveRetryPolicy.MinBudgetForRetryMs);
    }

    [Theory]
    [InlineData(WireConstants.FallbackRateLimited)]
    [InlineData(WireConstants.FallbackProtocolError)]
    [InlineData(WireConstants.OgFallbackReasonResolverUnhealthy)]
    [InlineData(WireConstants.OgFallbackReasonResolvedUrlRejected)]
    public void ServerControlAndGateReasons_AreNotRetryable(string reason)
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(reason, 0, 30_000));
    }

    [Fact]
    public void RetryDelayMs_small_hint_is_honored_as_the_delay()
    {
        Assert.Equal(2000, ResolveRetryPolicy.RetryDelayMs(
            WireConstants.FallbackDiscoveryInProgress, retryAfterMs: 2000, attempt: 0, remainingBudgetMs: 24000));
    }

    [Fact]
    public void RetryDelayMs_large_hint_stops_retrying()
    {
        Assert.Null(ResolveRetryPolicy.RetryDelayMs(
            WireConstants.FallbackDiscoveryInProgress, retryAfterMs: 25000, attempt: 0, remainingBudgetMs: 26000));
    }

    [Fact]
    public void RetryDelayMs_without_a_hint_keeps_the_blind_ladder()
    {
        Assert.Equal(750, ResolveRetryPolicy.RetryDelayMs(
            WireConstants.FallbackDiscoveryInProgress, retryAfterMs: null, attempt: 0, remainingBudgetMs: 26000));
    }

    [Fact]
    public void RetryDelayMs_hint_that_would_strand_the_ladder_stops_retrying()
    {
        Assert.Null(ResolveRetryPolicy.RetryDelayMs(
            WireConstants.FallbackDiscoveryInProgress, retryAfterMs: 4800, attempt: 0, remainingBudgetMs: 6000));
    }

    [Fact]
    public void RetryDelayMs_other_reasons_ignore_the_hint()
    {
        Assert.Equal(2250, ResolveRetryPolicy.RetryDelayMs(
            WireConstants.FallbackServerUnreachable, retryAfterMs: 25000, attempt: 1, remainingBudgetMs: 26000));
    }
}
