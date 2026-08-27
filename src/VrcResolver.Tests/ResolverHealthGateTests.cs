using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public sealed class ResolverHealthGateTests
{
    private static readonly TimeSpan Cooldown = TimeSpan.FromSeconds(30);
    private static readonly TimeSpan Verdict = TimeSpan.FromSeconds(25);

    private static ResolverHealthGate MakeGate(Func<DateTime> now)
        => new(Cooldown, Verdict, now);

    private static DateTime T0 => new(2026, 8, 21, 12, 0, 0, DateTimeKind.Utc);

    [Fact]
    public void OpensOnThirdConsecutiveResolveFailure()
    {
        var now = T0;
        var gate = MakeGate(() => now);

        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordResolveOutcome(healthy: false, resolved: false));
        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordResolveOutcome(healthy: false, resolved: false));
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.Equal(ResolverHealthGate.Transition.Opened, gate.RecordResolveOutcome(healthy: false, resolved: false));
        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void HealthyVerdictResetsResolveStreak()
    {
        var now = T0;
        var gate = MakeGate(() => now);

        gate.RecordResolveOutcome(healthy: false, resolved: false);
        gate.RecordResolveOutcome(healthy: false, resolved: false);
        gate.RecordResolveOutcome(healthy: true, resolved: true);
        gate.RecordResolveOutcome(healthy: false, resolved: false);
        gate.RecordResolveOutcome(healthy: false, resolved: false);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.Equal(ResolverHealthGate.Transition.Opened, gate.RecordResolveOutcome(healthy: false, resolved: false));
    }

    [Fact]
    public void NoProbeWhileMeshDisconnected()
    {
        var now = T0;
        var gate = MakeGate(() => now);
        Open(gate);

        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.True(gate.ShouldShortCircuit(meshConnected: false, out _));
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void CooldownGatesTheProbe_AndConcurrentCallsStayShortCircuited()
    {
        var now = T0;
        var gate = MakeGate(() => now);
        Open(gate);

        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));
        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void UnhealthyProbeReopensWithFreshCooldown()
    {
        var now = T0;
        var gate = MakeGate(() => now);
        Open(gate);

        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordResolveOutcome(healthy: false, resolved: false));

        now += TimeSpan.FromSeconds(29);
        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));
        now += TimeSpan.FromSeconds(2);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void HealthyFallbackProbeClosesImmediately()
    {
        var now = T0;
        var gate = MakeGate(() => now);
        Open(gate);

        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.Equal(ResolverHealthGate.Transition.Closed, gate.RecordResolveOutcome(healthy: true, resolved: false));
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void ResolvedProbeWaitsForPlaybackConfirmation()
    {
        var now = T0;
        var gate = MakeGate(() => now);
        Open(gate);

        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordResolveOutcome(healthy: true, resolved: true));
        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.Equal(ResolverHealthGate.Transition.Closed, gate.RecordPlaybackConfirmed());
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void ResolvedProbePlaybackFailureReopens()
    {
        var now = T0;
        var gate = MakeGate(() => now);
        Open(gate);

        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        gate.RecordResolveOutcome(healthy: true, resolved: true);
        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordPlaybackFailure());
        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));

        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void VerdictTimeoutClosesTheGate()
    {
        var now = T0;
        var gate = MakeGate(() => now);
        Open(gate);

        now += Cooldown + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        gate.RecordResolveOutcome(healthy: true, resolved: true);
        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));

        now += Verdict + TimeSpan.FromSeconds(1);
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out var transition));
        Assert.Equal(ResolverHealthGate.Transition.Closed, transition);
    }

    [Fact]
    public void PlaybackStreakOpensDespiteHealthyResolves()
    {
        var now = T0;
        var gate = MakeGate(() => now);

        gate.RecordResolveOutcome(healthy: true, resolved: true);
        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordPlaybackFailure());
        gate.RecordResolveOutcome(healthy: true, resolved: true);
        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordPlaybackFailure());
        gate.RecordResolveOutcome(healthy: true, resolved: true);
        Assert.Equal(ResolverHealthGate.Transition.Opened, gate.RecordPlaybackFailure());
        Assert.True(gate.ShouldShortCircuit(meshConnected: true, out _));
    }

    [Fact]
    public void ConfirmedPlaybackResetsPlaybackStreak()
    {
        var now = T0;
        var gate = MakeGate(() => now);

        gate.RecordPlaybackFailure();
        gate.RecordPlaybackFailure();
        Assert.Equal(ResolverHealthGate.Transition.None, gate.RecordPlaybackConfirmed());
        gate.RecordPlaybackFailure();
        gate.RecordPlaybackFailure();
        Assert.False(gate.ShouldShortCircuit(meshConnected: true, out _));
        Assert.Equal(ResolverHealthGate.Transition.Opened, gate.RecordPlaybackFailure());
    }

    [Theory]
    [InlineData(WireConstants.OgFallbackReasonResolverUnhealthy)]
    [InlineData(WireConstants.OgFallbackReasonResolvedUrlRejected)]
    public void GateReasonsAreNotRetryable(string reason)
    {
        Assert.False(ResolveRetryPolicy.ShouldRetry(reason, 0, 20_000));
    }

    private static void Open(ResolverHealthGate gate)
    {
        gate.RecordResolveOutcome(healthy: false, resolved: false);
        gate.RecordResolveOutcome(healthy: false, resolved: false);
        Assert.Equal(ResolverHealthGate.Transition.Opened, gate.RecordResolveOutcome(healthy: false, resolved: false));
    }
}
