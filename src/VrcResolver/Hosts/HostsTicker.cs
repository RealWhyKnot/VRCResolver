using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed class HostsTicker : IDisposable
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromMinutes(1);
    private static readonly TimeSpan ReAddBackoff = TimeSpan.FromMinutes(10);

    private readonly CancellationTokenSource _cts = new();
    private Task? _runner;

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
            try { await _runner.ConfigureAwait(false); } catch { }
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

        if (_lastPresent != false)
        {
            ConsoleUx.Write(LogComponent.Hosts, "tick: " + HostsManager.MarkerHost + " missing -- re-adding");
            _lastPresent = false;
        }

        if (DateTime.UtcNow < _nextReAddAttemptUtc)
        {
            return;
        }
        _nextReAddAttemptUtc = DateTime.UtcNow + ReAddBackoff;

        var sw = Stopwatch.StartNew();
        try
        {
            HostsManager.EnsureBypassEntryOrPrompt();
            sw.Stop();
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
