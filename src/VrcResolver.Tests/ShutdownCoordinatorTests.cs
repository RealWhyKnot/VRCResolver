using System.Collections.Concurrent;
using System.Diagnostics;
using Xunit;

namespace VrcResolver.Tests;

public class ShutdownCoordinatorTests
{
    private static readonly TimeSpan Generous = TimeSpan.FromSeconds(5);

    private readonly ConcurrentQueue<string> _log = new();

    private ShutdownCoordinator NewCoordinator(Func<string, bool> restore, Func<IReadOnlyList<ShutdownStep>> steps)
        => new(restore, steps, _log.Enqueue);

    private static ShutdownStep Step(string name, Func<Task> run, int budgetMs = 1000)
        => new(name, run, TimeSpan.FromMilliseconds(budgetMs));

    [Fact]
    public async Task Second_caller_gets_the_same_task_and_waits_for_the_restore()
    {
        var gate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var coordinator = NewCoordinator(_ =>
        {
            gate.Task.Wait();
            return true;
        }, () => []);

        Task first = coordinator.RunAsync("ctrl-c", fast: false);
        Task second = coordinator.RunAsync("quit", fast: false);

        Assert.Same(first, second);
        await Task.Delay(150);
        Assert.False(second.IsCompleted);

        gate.SetResult();
        await second.WaitAsync(Generous);

        Assert.Contains("shutdown trigger=ctrl-c", _log);
        Assert.DoesNotContain(_log, line => line.Contains("quit"));
    }

    [Fact]
    public async Task Restore_runs_before_slow_steps()
    {
        var clock = new Stopwatch();
        var order = new ConcurrentQueue<string>();
        long restoredAtMs = -1;
        var restored = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var coordinator = NewCoordinator(_ =>
        {
            Interlocked.Exchange(ref restoredAtMs, clock.ElapsedMilliseconds);
            order.Enqueue("restore");
            restored.SetResult();
            return true;
        }, () => [Step("slow", () =>
        {
            order.Enqueue("slow");
            return Task.Delay(TimeSpan.FromSeconds(5));
        }, budgetMs: 10_000)]);

        clock.Start();
        _ = coordinator.RunAsync("quit", fast: false);
        await restored.Task.WaitAsync(Generous);

        Assert.InRange(Interlocked.Read(ref restoredAtMs), 0, 100);
        Assert.Equal("restore", order.First());
    }

    [Fact]
    public async Task Fast_shutdown_restores_and_skips_steps()
    {
        bool restored = false;
        bool stepRan = false;
        var coordinator = NewCoordinator(_ =>
        {
            restored = true;
            return true;
        }, () => [Step("terminal", () =>
        {
            stepRan = true;
            return Task.CompletedTask;
        })]);

        await coordinator.RunAsync("console-close", fast: true).WaitAsync(Generous);

        Assert.True(restored);
        Assert.False(stepRan);
        Assert.Equal(new[] { "shutdown trigger=console-close", "tools restored=true" }, _log);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task Restore_failure_is_logged_and_steps_still_run(bool restoreThrows)
    {
        bool stepRan = false;
        var coordinator = NewCoordinator(_ =>
        {
            if (restoreThrows) throw new IOException("tools locked");
            return false;
        }, () => [Step("ipc", () =>
        {
            stepRan = true;
            return Task.CompletedTask;
        })]);

        await coordinator.RunAsync("quit", fast: false).WaitAsync(Generous);

        Assert.True(stepRan);
        Assert.Contains("tools restored=false", _log);
        if (restoreThrows)
            Assert.Contains(_log, line => line.Contains("IOException") && line.Contains("tools locked"));
    }

    [Fact]
    public async Task Step_over_budget_does_not_block_later_steps()
    {
        var never = new TaskCompletionSource();
        bool laterRan = false;
        var coordinator = NewCoordinator(_ => true, () =>
        [
            Step("mesh", () => never.Task, budgetMs: 100),
            Step("resolve-cache", () =>
            {
                laterRan = true;
                return Task.CompletedTask;
            }),
        ]);

        var clock = Stopwatch.StartNew();
        await coordinator.RunAsync("quit", fast: false).WaitAsync(Generous);

        Assert.True(laterRan);
        Assert.InRange(clock.ElapsedMilliseconds, 0, 2000);
        Assert.Contains(_log, line => line.Contains("mesh") && line.Contains("100 ms"));
    }

    [Fact]
    public async Task Step_that_throws_does_not_block_later_steps()
    {
        bool laterRan = false;
        var coordinator = NewCoordinator(_ => true, () =>
        [
            Step("relay", () => throw new InvalidOperationException("listener gone")),
            Step("mesh", () =>
            {
                laterRan = true;
                return Task.CompletedTask;
            }),
        ]);

        await coordinator.RunAsync("quit", fast: false).WaitAsync(Generous);

        Assert.True(laterRan);
        Assert.Contains(_log, line => line.Contains("relay") && line.Contains("listener gone"));
    }

    [Fact]
    public async Task Shutdown_before_steps_exist_still_restores()
    {
        List<ShutdownStep>? services = null;
        string? restoreReason = null;
        var coordinator = NewCoordinator(reason =>
        {
            restoreReason = reason;
            return true;
        }, () => services ?? []);

        await coordinator.RunAsync("ctrl-c", fast: false).WaitAsync(Generous);

        Assert.Equal("ctrl-c", restoreReason);
        Assert.Equal(new[] { "shutdown trigger=ctrl-c", "tools restored=true" }, _log);
    }

    [Fact]
    public async Task Steps_are_read_when_the_shutdown_starts()
    {
        List<ShutdownStep>? services = null;
        bool stepRan = false;
        var coordinator = NewCoordinator(_ => true, () => services ?? []);

        services = [Step("heartbeat", () =>
        {
            stepRan = true;
            return Task.CompletedTask;
        })];
        await coordinator.RunAsync("quit", fast: false).WaitAsync(Generous);

        Assert.True(stepRan);
    }
}
