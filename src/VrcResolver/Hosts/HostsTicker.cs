using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed class HostsTicker : IDisposable
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromMinutes(1);
    private const string DeclinedReason = "UAC declined or write blocked";

    private readonly Func<(bool Ok, bool Present, string? Error)> _readState;
    private readonly Action _ensureEntry;
    private readonly Func<bool> _isActive;
    private readonly Func<bool> _promptInFlight;
    private readonly Func<bool> _promptDeclined;
    private readonly Func<DateTime> _nowUtc;

    private readonly CancellationTokenSource _cts = new();
    private Task? _runner;

    private bool? _lastPresent;
    private bool _gaveUp;

    public HostsTicker()
        : this(
            ReadHostsState,
            HostsManager.EnsureBypassEntryOrPrompt,
            HostsManager.IsBypassActive,
            () => HostsManager.IsPromptInFlight,
            () => HostsManager.PromptDeclinedThisSession,
            () => DateTime.UtcNow)
    {
    }

    internal HostsTicker(
        Func<(bool Ok, bool Present, string? Error)> readState,
        Action ensureEntry,
        Func<bool> isActive,
        Func<bool> promptInFlight,
        Func<bool> promptDeclined,
        Func<DateTime> nowUtc)
    {
        _readState = readState;
        _ensureEntry = ensureEntry;
        _isActive = isActive;
        _promptInFlight = promptInFlight;
        _promptDeclined = promptDeclined;
        _nowUtc = nowUtc;
    }

    private static (bool Ok, bool Present, string? Error) ReadHostsState()
    {
        bool ok = HostsManager.TryReadBypassState(out bool present, out string? error);
        return (ok, present, error);
    }

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

    internal void Tick()
    {
        var (ok, present, error) = _readState();
        if (!ok)
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

        if (_gaveUp)
        {
            _lastPresent = false;
            return;
        }

        if (_promptInFlight())
        {
            return;
        }

        if (_promptDeclined())
        {
            _lastPresent = false;
            GiveUp(DeclinedReason);
            return;
        }

        if (_lastPresent != false)
        {
            ConsoleUx.Write(LogComponent.Hosts, "tick: " + HostsManager.MarkerHost + " missing -- re-adding");
            _lastPresent = false;
        }

        DateTime startedUtc = _nowUtc();
        try
        {
            _ensureEntry();
            if (_isActive())
            {
                ConsoleUx.Write(LogComponent.Hosts, "tick: re-add succeeded in " + (long)(_nowUtc() - startedUtc).TotalMilliseconds + " ms");
                _lastPresent = true;
                return;
            }

            if (_promptInFlight())
            {
                return;
            }
            GiveUp(DeclinedReason);
        }
        catch (Exception ex)
        {
            Logger.WriteFileOnly("[hosts] re-add threw: " + ex.GetType().Name + ": " + ex.Message);
            GiveUp(ex.GetType().Name);
        }
    }

    private void GiveUp(string reason)
    {
        _gaveUp = true;
        ConsoleUx.Warn(LogComponent.Hosts, "tick: re-add failed (" + reason + ") -- won't ask again until the next start");
    }

    public void Dispose()
    {
        _cts.Cancel();
        _cts.Dispose();
    }
}
