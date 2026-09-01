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
}
