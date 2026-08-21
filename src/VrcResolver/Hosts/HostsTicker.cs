using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

// Periodic verifier for the localhost.youtube.com hosts entry. The watchdog
// adds the entry once on startup via HostsManager.EnsureBypassEntryOrPrompt,
// but it can disappear: manual hand-edit, OS rollback, antivirus rewrite,
// driver update reset — all observed in the wild. This ticker re-checks
// every minute and re-adds if missing.
//
// State-change-gated logging mirrors PatchManager's tick logging: emit on
// transition (was-present → missing → re-add succeeded) and stay silent on
// matched ticks. UAC re-prompt is rate-limited to once per 10 minutes so a
// user who declines doesn't get spammed by the prompt loop.
[SupportedOSPlatform("windows")]
internal sealed class HostsTicker : IDisposable
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromMinutes(1);
    private static readonly TimeSpan ReAddBackoff = TimeSpan.FromMinutes(10);

    private readonly CancellationTokenSource _cts = new();
    private Task? _runner;

    // null = first tick (no previous state to compare); true/false = last
    // observed presence state. Drives the state-change gate so steady-state
    // ticks don't spam the console.
    private bool? _lastPresent;
    private DateTime _nextReAddAttemptUtc = DateTime.MinValue;

    public void Start()
    {
        _runner = Task.Run(() => RunAsync(_cts.Token));
    }

    public async Task StopAsync()
    {
        _cts.Cancel();
        if (_runner != null)
        {
            try { await _runner.ConfigureAwait(false); } catch { /* ignore */ }
        }
    }

    private async Task RunAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            try { Tick(); }
            catch (Exception ex)
            {
                ConsoleUx.Warn(LogComponent.Hosts, "tick threw: " + ex.GetType().Name + ": " + ex.Message);
            }
            try { await Task.Delay(TickInterval, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }
        }
    }

    private void Tick()
    {
        if (!HostsManager.TryReadBypassState(out bool present, out string? error))
        {
            // Couldn't read hosts file at all (locked / permissions / weird
            // state). Don't trigger a re-add — that would UAC-prompt for a
            // condition we can't verify. Log once per state change.
            if (_lastPresent != null)
            {
                ConsoleUx.Warn(LogComponent.Hosts, "tick: hosts file unreadable (" + error + ") -- skipping check");
                _lastPresent = null;
            }
            return;
        }

        if (present)
        {
            if (_lastPresent != true)
            {
                ConsoleUx.Write(LogComponent.Hosts, "tick: " + HostsManager.MarkerHost + " entry present");
                _lastPresent = true;
            }
            return;
        }

        // Entry missing. Log once per transition; respect UAC re-prompt
        // backoff so user-declined-prompt doesn't loop every minute.
        if (_lastPresent != false)
        {
            ConsoleUx.Write(LogComponent.Hosts, "tick: " + HostsManager.MarkerHost + " missing -- re-adding");
            _lastPresent = false;
        }

        if (DateTime.UtcNow < _nextReAddAttemptUtc)
        {
            // Within UAC backoff window — already prompted recently and was
            // denied or failed. Stay quiet until the window expires.
            return;
        }
        _nextReAddAttemptUtc = DateTime.UtcNow + ReAddBackoff;

        var sw = Stopwatch.StartNew();
        try
        {
            HostsManager.EnsureBypassEntryOrPrompt();
            sw.Stop();
            // Re-check; EnsureBypassEntryOrPrompt logs its own success line,
            // but we want the tick path to confirm the post-state too.
            if (HostsManager.IsBypassActive())
            {
                ConsoleUx.Write(LogComponent.Hosts, "tick: re-add succeeded in " + sw.ElapsedMilliseconds + " ms");
                _lastPresent = true;
            }
            else
            {
                ConsoleUx.Warn(LogComponent.Hosts, "tick: re-add failed (UAC declined or write blocked) -- next attempt in " + (int)ReAddBackoff.TotalMinutes + " min");
            }
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Hosts, "tick re-add threw: " + ex.GetType().Name + ": " + ex.Message);
        }
    }

    public void Dispose()
    {
        _cts.Cancel();
        _cts.Dispose();
    }
}
