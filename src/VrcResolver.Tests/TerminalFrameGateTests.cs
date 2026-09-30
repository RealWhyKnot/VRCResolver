using System.Runtime.Versioning;
using VrcResolver;
using Xunit;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
public sealed class TerminalFrameGateTests
{
    private static readonly DateTime Now = new(2026, 9, 30, 12, 0, 0, DateTimeKind.Utc);

    private static WatchdogActivitySnapshot Snapshot(DateTime? lastRelay = null)
    {
        return new WatchdogActivitySnapshot(1, 0, 0, 0, 0, 0, 100, 0, 0, lastRelay, null);
    }

    private static WatchdogBandwidthSnapshot Bandwidth(long current = 0)
    {
        return new WatchdogBandwidthSnapshot(current, current, new long[24]);
    }

    private static bool Query(
        TerminalFrameGate gate,
        WatchdogActivitySnapshot snapshot,
        WatchdogBandwidthSnapshot bandwidth,
        DateTime now,
        int spinner = 0,
        int width = 120,
        string input = "",
        bool animations = true)
    {
        return gate.TryGetCached(snapshot, bandwidth, now, true, spinner, width, input, true, animations, true, "", -1, out _);
    }

    [Fact]
    public void IdleRepeat_ReusesStoredFrameEvenWhenSpinnerAdvances()
    {
        var gate = new TerminalFrameGate();
        Assert.False(Query(gate, Snapshot(), Bandwidth(), Now, spinner: 1));
        gate.Store(TerminalFrame.Plain("frame", ConsoleColor.Gray));

        Assert.True(Query(gate, Snapshot(), Bandwidth(), Now.AddSeconds(1), spinner: 2));
        Assert.True(Query(gate, Snapshot(), Bandwidth(), Now.AddSeconds(2), spinner: 3));
    }

    [Fact]
    public void ActiveRelay_SpinnerAdvanceInvalidates()
    {
        var gate = new TerminalFrameGate();
        var snapshot = Snapshot(Now);
        Assert.False(Query(gate, snapshot, Bandwidth(10), Now, spinner: 1));
        gate.Store(TerminalFrame.Plain("frame", ConsoleColor.Gray));

        Assert.False(Query(gate, snapshot, Bandwidth(10), Now, spinner: 2));
    }

    [Fact]
    public void ActivityWindowExpiry_Invalidates()
    {
        var gate = new TerminalFrameGate();
        var snapshot = Snapshot(Now);
        Assert.False(Query(gate, snapshot, Bandwidth(), Now));
        gate.Store(TerminalFrame.Plain("frame", ConsoleColor.Gray));
        Assert.True(Query(gate, snapshot, Bandwidth(), Now.AddSeconds(1)));

        Assert.False(Query(gate, snapshot, Bandwidth(), Now.AddSeconds(3)));
    }

    [Fact]
    public void WidthOrInputChange_Invalidates()
    {
        var gate = new TerminalFrameGate();
        Assert.False(Query(gate, Snapshot(), Bandwidth(), Now));
        gate.Store(TerminalFrame.Plain("frame", ConsoleColor.Gray));

        Assert.False(Query(gate, Snapshot(), Bandwidth(), Now, width: 80));
        gate.Store(TerminalFrame.Plain("frame", ConsoleColor.Gray));
        Assert.False(Query(gate, Snapshot(), Bandwidth(), Now, width: 80, input: "x"));
    }

    [Fact]
    public void HistoryChange_Invalidates()
    {
        var gate = new TerminalFrameGate();
        Assert.False(Query(gate, Snapshot(), Bandwidth(), Now));
        gate.Store(TerminalFrame.Plain("frame", ConsoleColor.Gray));

        var history = new long[24];
        history[3] = 5;
        Assert.False(Query(gate, Snapshot(), new WatchdogBandwidthSnapshot(0, 5, history), Now));
    }

    [Fact]
    public void UnstoredFrame_IsNotReused()
    {
        var gate = new TerminalFrameGate();
        Assert.False(Query(gate, Snapshot(), Bandwidth(), Now));
        Assert.False(Query(gate, Snapshot(), Bandwidth(), Now));
    }
}
