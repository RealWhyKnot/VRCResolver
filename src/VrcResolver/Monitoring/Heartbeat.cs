using System.Globalization;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed class Heartbeat : IDisposable
{
    private static readonly TimeSpan Interval = TimeSpan.FromMinutes(30);
    private static readonly TimeSpan QuietWindow = TimeSpan.FromMinutes(5);

    private readonly MeshClient _mesh;
    private readonly ResolveCache? _cache;
    private readonly CancellationTokenSource _cts = new();
    private Task? _runner;

    public Heartbeat(MeshClient mesh, ResolveCache? cache = null)
    {
        _mesh = mesh;
        _cache = cache;
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
            try { await Task.Delay(Interval, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }
            try { Tick(); }
            catch (Exception ex)
            {
                ConsoleUx.Warn(LogComponent.Heartbeat, "tick threw: " + ex.GetType().Name + ": " + ex.Message);
            }
        }
    }

    private void Tick()
    {
        if (DateTime.UtcNow - Logger.LastWriteUtc < QuietWindow)
            return;

        TimeSpan up = DateTime.UtcNow - WatchdogStats.StartUtc;
        long resolves = WatchdogStats.ResolvesTotal;
        long lhYt = WatchdogStats.ResolvesViaLhYt;
        long cacheHits = WatchdogStats.ResolvesCacheHits;
        long bytes = WatchdogStats.BytesEstimateTotal;
        long reconnects = WatchdogStats.ReconnectCount;
        int cacheSize = _cache?.Count ?? 0;

        string meshState = _mesh.IsConnected ? "connected" : "disconnected";

        var sb = new System.Text.StringBuilder();
        sb.Append("up=").Append(FormatUptime(up));
        sb.Append(" mesh=").Append(meshState);
        if (resolves > 0)
        {
            sb.Append(" resolves=").Append(resolves);
            if (lhYt > 0) sb.Append(" (").Append(lhYt).Append(" local video)");
        }
        if (bytes > 0) sb.Append(" video=").Append(WatchdogDisplay.FormatBytes(bytes));
        if (cacheSize > 0 || cacheHits > 0)
            sb.Append(" cache=").Append(cacheSize).Append('(').Append(cacheHits).Append("hits)");
        if (reconnects > 0) sb.Append(" reconnects=").Append(reconnects);

        ConsoleUx.Write(LogComponent.Heartbeat, sb.ToString());
    }

    internal static string FormatUptime(TimeSpan ts)
    {
        if (ts.TotalDays >= 1)
            return ((int)ts.TotalDays).ToString(CultureInfo.InvariantCulture)
                + "d" + ts.Hours.ToString(CultureInfo.InvariantCulture) + "h";
        if (ts.TotalHours >= 1)
            return ((int)ts.TotalHours).ToString(CultureInfo.InvariantCulture)
                + "h" + ts.Minutes.ToString(CultureInfo.InvariantCulture) + "m";
        return ((int)ts.TotalMinutes).ToString(CultureInfo.InvariantCulture) + "m";
    }

    internal static string FormatBytes(long bytes) => WatchdogDisplay.FormatBytes(bytes);

    public void Dispose()
    {
        _cts.Cancel();
        _cts.Dispose();
    }
}
