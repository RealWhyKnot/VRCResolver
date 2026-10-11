using System.Diagnostics;

namespace VrcResolver;

internal sealed record ShutdownStep(string Name, Func<Task> Run, TimeSpan Budget);

internal sealed class ShutdownCoordinator
{
    private static readonly TimeSpan TotalBudget = TimeSpan.FromSeconds(12);

    private readonly Func<string, bool> _restoreTools;
    private readonly Func<IReadOnlyList<ShutdownStep>> _steps;
    private readonly Action<string> _log;
    private readonly object _gate = new();
    private Task? _run;

    public ShutdownCoordinator(Func<string, bool> restoreTools, Func<IReadOnlyList<ShutdownStep>> steps, Action<string> log)
    {
        _restoreTools = restoreTools;
        _steps = steps;
        _log = log;
    }

    public Task RunAsync(string trigger, bool fast)
    {
        lock (_gate)
        {
            return _run ??= Task.Factory.StartNew(
                () => RunCoreAsync(trigger, fast),
                CancellationToken.None,
                TaskCreationOptions.LongRunning,
                TaskScheduler.Default).Unwrap();
        }
    }

    private async Task RunCoreAsync(string trigger, bool fast)
    {
        var clock = Stopwatch.StartNew();
        Log("shutdown trigger=" + trigger);

        bool restored = false;
        try { restored = _restoreTools(trigger); }
        catch (Exception ex) { Log("tools restore threw " + ex.GetType().Name + ": " + ex.Message); }
        Log("tools restored=" + (restored ? "true" : "false"));

        if (fast) return;

        IReadOnlyList<ShutdownStep> steps;
        try { steps = _steps(); }
        catch (Exception ex)
        {
            Log("shutdown steps unavailable: " + ex.GetType().Name + ": " + ex.Message);
            return;
        }

        foreach (ShutdownStep step in steps)
        {
            TimeSpan remaining = TotalBudget - clock.Elapsed;
            if (remaining <= TimeSpan.Zero)
            {
                Log("shutdown budget spent; skipping " + step.Name + " and the rest");
                return;
            }
            await RunStepAsync(step, step.Budget < remaining ? step.Budget : remaining).ConfigureAwait(false);
        }
    }

    private async Task RunStepAsync(ShutdownStep step, TimeSpan budget)
    {
        Task run = Task.Run(step.Run);
        try
        {
            await run.WaitAsync(budget).ConfigureAwait(false);
        }
        catch (TimeoutException) when (!run.IsCompleted)
        {
            Log("shutdown step " + step.Name + " took longer than " + (long)budget.TotalMilliseconds + " ms; moving on");
            _ = run.ContinueWith(
                static t => _ = t.Exception,
                CancellationToken.None,
                TaskContinuationOptions.OnlyOnFaulted | TaskContinuationOptions.ExecuteSynchronously,
                TaskScheduler.Default);
        }
        catch (Exception ex)
        {
            Log("shutdown step " + step.Name + " failed: " + ex.GetType().Name + ": " + ex.Message);
        }
    }

    private void Log(string line)
    {
        try { _log(line); }
        catch { }
    }
}
