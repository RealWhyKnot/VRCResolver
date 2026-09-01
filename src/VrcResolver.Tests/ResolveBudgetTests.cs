using System;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class ResolveBudgetTests
{
    [Fact]
    public void OgWindow_when_rung1_consumed_the_budget_falls_back_to_the_reserve()
    {
        Assert.Equal(ResolveBudget.OgReserve,
            ResolveBudget.OgWindow(TimeSpan.FromMilliseconds(26500), reAskAvailable: true));
    }

    [Fact]
    public void OgWindow_after_a_fast_decline_covers_the_rest_of_the_budget()
    {
        var window = ResolveBudget.OgWindow(TimeSpan.FromMilliseconds(100), reAskAvailable: true);
        Assert.Equal(TimeSpan.FromMilliseconds(22900), window);
    }

    [Fact]
    public void OgWindow_never_drops_below_the_og_reserve()
    {
        Assert.Equal(ResolveBudget.OgReserve,
            ResolveBudget.OgWindow(TimeSpan.FromSeconds(27), reAskAvailable: true));
        Assert.Equal(ResolveBudget.OgReserve,
            ResolveBudget.OgWindow(TimeSpan.FromSeconds(600), reAskAvailable: true));
    }

    [Fact]
    public void OgWindow_without_a_re_ask_keeps_the_hard_cap()
    {
        Assert.Equal(ResolveBudget.OgHardCap,
            ResolveBudget.OgWindow(TimeSpan.FromSeconds(26), reAskAvailable: false));
    }

    [Fact]
    public void OgWindow_after_a_fast_decline_still_leaves_the_re_ask_above_its_minimum()
    {
        var elapsed = TimeSpan.FromMilliseconds(100);
        var window = ResolveBudget.OgWindow(elapsed, reAskAvailable: true);
        var left = ResolveBudget.Total - elapsed - window;
        Assert.True(left.TotalMilliseconds >= ResolveRetryPolicy.MinBudgetForRetryMs);
    }

    [Fact]
    public void OgWindow_inherits_the_re_ask_reserve_when_the_hint_cannot_fit()
    {
        Assert.Equal(TimeSpan.FromMilliseconds(12000),
            ResolveBudget.OgWindow(TimeSpan.FromSeconds(16), reAskAvailable: true, retryAtElapsedMs: 41500));
    }

    [Fact]
    public void OgWindow_keeps_the_re_ask_reserve_when_the_hint_fits()
    {
        Assert.Equal(TimeSpan.FromMilliseconds(7000),
            ResolveBudget.OgWindow(TimeSpan.FromSeconds(16), reAskAvailable: true, retryAtElapsedMs: 20000));
    }

    [Fact]
    public void OgWindow_with_an_unfittable_hint_still_floors_at_the_reserve()
    {
        Assert.Equal(ResolveBudget.OgReserve,
            ResolveBudget.OgWindow(TimeSpan.FromMilliseconds(26500), reAskAvailable: true, retryAtElapsedMs: 41500));
    }

    [Fact]
    public void OgWindow_without_a_re_ask_ignores_the_hint()
    {
        Assert.Equal(ResolveBudget.OgHardCap,
            ResolveBudget.OgWindow(TimeSpan.FromSeconds(26), reAskAvailable: false, retryAtElapsedMs: 41500));
    }

    [Fact]
    public void ReAskDelayMs_without_a_hint_re_asks_immediately_while_budget_remains()
    {
        Assert.Equal(0L, ResolveBudget.ReAskDelayMs(elapsedMs: 20000, retryAtElapsedMs: null));
    }

    [Fact]
    public void ReAskDelayMs_without_a_hint_skips_below_the_floor()
    {
        Assert.Null(ResolveBudget.ReAskDelayMs(elapsedMs: 25500, retryAtElapsedMs: null));
    }

    [Fact]
    public void ReAskDelayMs_with_a_passed_hint_re_asks_immediately()
    {
        Assert.Equal(0L, ResolveBudget.ReAskDelayMs(elapsedMs: 17000, retryAtElapsedMs: 8000));
    }

    [Fact]
    public void ReAskDelayMs_with_a_pending_hint_waits_only_the_remainder()
    {
        Assert.Equal(500L, ResolveBudget.ReAskDelayMs(elapsedMs: 4500, retryAtElapsedMs: 5000));
    }

    [Fact]
    public void ReAskDelayMs_with_an_unfittable_hint_skips()
    {
        Assert.Null(ResolveBudget.ReAskDelayMs(elapsedMs: 16500, retryAtElapsedMs: 41500));
    }

    [Fact]
    public void ReAskDelayMs_boundary_hint_that_exactly_fits_is_honored()
    {
        Assert.Equal(25650L, ResolveBudget.ReAskDelayMs(elapsedMs: 100, retryAtElapsedMs: 25750));
    }

    [Fact]
    public void ReAskDelayMs_with_a_stale_hint_judges_viability_from_elapsed()
    {
        Assert.Null(ResolveBudget.ReAskDelayMs(elapsedMs: 26500, retryAtElapsedMs: 8000));
    }
}
