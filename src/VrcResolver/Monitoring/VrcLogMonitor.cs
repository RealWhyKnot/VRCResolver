using System.Runtime.Versioning;
using System.Text.RegularExpressions;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed partial class VrcLogMonitor : IDisposable
{
    private static readonly TimeSpan AvProOpenFailCorrelationWindow = TimeSpan.FromSeconds(10);
    private static readonly TimeSpan SilentStallWindow = TimeSpan.FromSeconds(12);
    private static readonly TimeSpan DeliveredHeightTtl = TimeSpan.FromMinutes(10);
    private static readonly TimeSpan PlayingFeedbackInterval = TimeSpan.FromSeconds(15);
    private const int MaxDeliveredHeightEntries = 64;

    [GeneratedRegex(@"\[AVProVideo\] Opening\s+(\S+)", RegexOptions.IgnoreCase)]
    private static partial Regex AvProOpeningRegex();
    [GeneratedRegex(@"\bSwitched\s+to(?:\s+Resolution)?\s+(\d{2,5})x(\d{2,5})\b", RegexOptions.IgnoreCase)]
    private static partial Regex SwitchedResolutionRegex();
    [GeneratedRegex(@"\bfwidth=(\d{2,5})\b.*\bfheight=(\d{2,5})\b", RegexOptions.IgnoreCase)]
    private static partial Regex AvProStateResolutionRegex();

    private readonly MeshClient _mesh;
    private readonly ResolveCache? _cache;
    private readonly OgFallbackHint? _ogFallbackHint;
    private readonly ResolverHealthGate? _health;
    private readonly CancellationTokenSource _cts = new();
    private Task? _loop;

    private string? _lastOpeningUrl;
    private DateTime _lastOpeningAt;
    private string? _activePlaybackUrl;
    private DateTime _activePlaybackAt;

    private CancellationTokenSource? _stallCts;
    private string? _stallUrl;
    private DateTime _stallAt;
    private readonly object _stallLock = new();
    private readonly object _deliveredHeightLock = new();
    private readonly Dictionary<string, (int Height, DateTime At)> _deliveredHeights = new();
    private CancellationTokenSource? _playingFeedbackCts;
    private string? _playingFeedbackUrl;

    public VrcLogMonitor(MeshClient mesh, ResolveCache? cache = null, OgFallbackHint? ogFallbackHint = null, ResolverHealthGate? health = null)
    {
        _mesh = mesh;
        _ogFallbackHint = ogFallbackHint;
        _cache = cache;
        _health = health;
    }

    private bool IsAttributedToUs(string canonicalUrl)
    {
        if (string.IsNullOrEmpty(canonicalUrl)) return false;
        return FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUrl(canonicalUrl)
            || (_cache != null && _cache.TryGetSourceUrlForResolved(canonicalUrl, out _));
    }

    private void ConfirmPlayback(string canonicalUrl)
    {
        if (_health == null || !IsAttributedToUs(canonicalUrl)) return;
        if (_health.RecordPlaybackConfirmed() == ResolverHealthGate.Transition.Closed)
            ConsoleUx.Success(LogComponent.VrcLog, "resolver restored -- mesh resolving re-enabled");
    }

    public void Start()
    {
        _loop = Task.Run(() => MonitorLoopAsync(_cts.Token));
    }

    public async Task StopAsync()
    {
        _cts.Cancel();
        CancelStallWatchdog();
        CancelPlayingFeedbackLoop();
        if (_loop != null)
        {
            try { await _loop.ConfigureAwait(false); } catch { }
        }
    }

    private async Task MonitorLoopAsync(CancellationToken ct)
    {
        string vrcDir = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData) + "Low",
            "VRChat", "VRChat");
        string currentFile = "";
        long lastSize = 0;

        while (!ct.IsCancellationRequested)
        {
            try
            {
                if (!Directory.Exists(vrcDir))
                {
                    try { await Task.Delay(5000, ct).ConfigureAwait(false); } catch { return; }
                    continue;
                }

                FileInfo? latest = null;
                try
                {
                    latest = new DirectoryInfo(vrcDir)
                        .GetFiles("output_log*.txt")
                        .OrderByDescending(f => f.LastWriteTime)
                        .FirstOrDefault();
                }
                catch { }

                if (latest != null)
                {
                    if (latest.FullName != currentFile)
                    {
                        bool firstFile = currentFile.Length == 0;
                        currentFile = latest.FullName;
                        lastSize = InitialReadOffsetForNewFile(latest.Length, firstFile);
                    }
                    if (latest.Length > lastSize)
                    {
                        try
                        {
                            using var fs = new FileStream(currentFile, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
                            fs.Seek(lastSize, SeekOrigin.Begin);
                            using var reader = new StreamReader(fs);
                            string newContent = await reader.ReadToEndAsync(ct).ConfigureAwait(false);
                            lastSize = fs.Position;
                            ProcessNewContent(newContent);
                        }
                        catch (IOException) { }
                    }
                    else if (latest.Length < lastSize)
                    {
                        lastSize = 0;
                    }
                }

                try { await Task.Delay(1000, ct).ConfigureAwait(false); } catch { return; }
            }
            catch (OperationCanceledException) { return; }
            catch (Exception ex)
            {
                ConsoleUx.Warn(LogComponent.VrcLog, "monitor error: " + ex.GetType().Name + ": " + ex.Message);
                try { await Task.Delay(5000, ct).ConfigureAwait(false); } catch { return; }
            }
        }
    }

    internal static long InitialReadOffsetForNewFile(long fileLength, bool firstFile)
    {
        return firstFile ? Math.Max(0, fileLength) : 0;
    }

    internal void ProcessNewContent(string content)
    {
        foreach (string rawLine in content.Split('\n'))
        {
            string line = rawLine.Trim();
            if (string.IsNullOrEmpty(line)) continue;

            var openingMatch = AvProOpeningRegex().Match(line);
            if (openingMatch.Success)
            {
                string opened = openingMatch.Groups[1].Value;
                string canonicalOpened = CanonicalPlaybackObservationUrl(opened);
                _lastOpeningUrl = opened;
                _lastOpeningAt = DateTime.UtcNow;
                _activePlaybackUrl = canonicalOpened;
                _activePlaybackAt = _lastOpeningAt;
                CancelPlayingFeedbackLoop();
                StartStallWatchdog(opened);
                if (TryGetDeliveredHeight(canonicalOpened, out _))
                    StartPlayingFeedbackLoop(canonicalOpened, _lastOpeningAt);
                continue;
            }

            if (TryParseObservedResolution(line, out _, out int observedHeight)
                && !string.IsNullOrEmpty(_activePlaybackUrl))
            {
                string activeUrl = _activePlaybackUrl!;
                DateTime activeAt = _activePlaybackAt == default ? DateTime.UtcNow : _activePlaybackAt;
                RememberDeliveredHeight(activeUrl, observedHeight);
                ConfirmPlayback(activeUrl);
                StartPlayingFeedbackLoop(activeUrl, activeAt);
                _ = _mesh.SendPlaybackFeedbackAsync(
                    activeUrl,
                    WireConstants.PlaybackFeedbackPlaying,
                    ElapsedMsSince(activeAt),
                    observedHeight);
                continue;
            }

            if (line.Contains("[AVProVideo] Error: Loading failed"))
            {
                if (_lastOpeningUrl != null
                    && DateTime.UtcNow - _lastOpeningAt <= AvProOpenFailCorrelationWindow)
                {
                    string observed = _lastOpeningUrl;
                    string failed = CanonicalPlaybackObservationUrl(observed);
                    int ms = (int)(DateTime.UtcNow - _lastOpeningAt).TotalMilliseconds;
                    _lastOpeningUrl = null;
                    int? deliveredHeight = TryGetDeliveredHeight(failed, out int height) ? height : null;
                    _ = _mesh.SendPlaybackFeedbackAsync(
                        failed,
                        WireConstants.PlaybackFeedbackLoadFailure,
                        ms,
                        deliveredHeight);
                    var fallback = MarkPlaybackFailure(failed);

                    ConsoleUx.Warn(LogComponent.VrcLog, "load_failure ms=" + ms + " url=" + LogUtil.RedactUrl(failed)
                        + (fallback.Evicted > 0 ? " evicted=" + fallback.Evicted : "")
                        + (fallback.OgHintArmed ? " og_hint=armed" : ""));
                }
                CancelStallWatchdog();
                CancelPlayingFeedbackLoop();
                _activePlaybackUrl = null;
                _activePlaybackAt = default;
                continue;
            }

            if (line.Contains("[AVProVideo] Using playback path:")
                || line.Contains("Now Playing:")
                || line.Contains("Load Url:"))
            {
                if (line.Contains("[AVProVideo] Using playback path:")
                    && _activePlaybackUrl is string acceptedUrl)
                {
                    ConfirmPlayback(acceptedUrl);
                }
                CancelStallWatchdog();
            }
        }
    }

    private void StartStallWatchdog(string url)
    {
        var newCts = new CancellationTokenSource();
        CancellationTokenSource? oldCts;
        DateTime now = DateTime.UtcNow;
        lock (_stallLock)
        {
            oldCts = _stallCts;
            _stallCts = newCts;
            _stallUrl = url;
            _stallAt = now;
        }
        try { oldCts?.Cancel(); oldCts?.Dispose(); } catch { }

        _ = Task.Run(async () =>
        {
            try { await Task.Delay(SilentStallWindow, newCts.Token).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }
            catch (ObjectDisposedException) { return; }

            string? activeUrl;
            DateTime activeAt;
            lock (_stallLock)
            {
                if (!ReferenceEquals(_stallCts, newCts)) return;
                activeUrl = _stallUrl;
                activeAt = _stallAt;
                _stallCts = null;
                _stallUrl = null;
            }
            try { newCts.Dispose(); } catch { }
            if (activeUrl == null) return;

            int ms = (int)(DateTime.UtcNow - activeAt).TotalMilliseconds;
            string reportedUrl = CanonicalPlaybackObservationUrl(activeUrl);
            int? deliveredHeight = TryGetDeliveredHeight(reportedUrl, out int height) ? height : null;
            _ = _mesh.SendPlaybackFeedbackAsync(
                reportedUrl,
                WireConstants.PlaybackFeedbackSilentStall,
                ms,
                deliveredHeight);
            CancelPlayingFeedbackLoop();
            _activePlaybackUrl = null;
            _activePlaybackAt = default;
            var fallback = MarkPlaybackFailure(reportedUrl);
            ConsoleUx.Warn(LogComponent.VrcLog, "silent_stall ms=" + ms + " url=" + LogUtil.RedactUrl(reportedUrl)
                + (fallback.Evicted > 0 ? " evicted=" + fallback.Evicted : "")
                + (fallback.OgHintArmed ? " og_hint=armed" : ""));
        });
    }

    internal PlaybackFailureRecovery MarkPlaybackFailureForTests(string reportedUrl) => MarkPlaybackFailure(reportedUrl);

    private PlaybackFailureRecovery MarkPlaybackFailure(string reportedUrl)
    {
        bool ogHintArmed = false;

        bool attributed = IsAttributedToUs(reportedUrl);
        if (_ogFallbackHint != null
            && _cache != null
            && _cache.TryGetSourceUrlForResolved(reportedUrl, out string sourceUrl))
        {
            _ogFallbackHint.RecordLoadFailure(sourceUrl);
            ogHintArmed = true;
        }

        int evicted = _cache?.EvictByUrl(reportedUrl) ?? 0;

        if (attributed && _health != null
            && _health.RecordPlaybackFailure() == ResolverHealthGate.Transition.Opened)
        {
            ConsoleUx.Warn(LogComponent.VrcLog,
                "resolver paused -- " + ResolverHealthGate.OpenThreshold
                + " playbacks failed in a row; VRChat's own resolver takes over while things recover");
        }

        return new PlaybackFailureRecovery(evicted, ogHintArmed);
    }

    internal static bool TryParseObservedResolution(string line, out int width, out int height)
    {
        width = 0;
        height = 0;
        if (string.IsNullOrWhiteSpace(line)) return false;

        var switched = SwitchedResolutionRegex().Match(line);
        if (switched.Success
            && TryParseResolution(switched.Groups[1].Value, switched.Groups[2].Value, out width, out height))
        {
            return true;
        }

        var state = AvProStateResolutionRegex().Match(line);
        return state.Success
            && TryParseResolution(state.Groups[1].Value, state.Groups[2].Value, out width, out height);
    }

    internal int? GetObservedDeliveredHeightForTests(string url)
    {
        string canonical = CanonicalPlaybackObservationUrl(url);
        return TryGetDeliveredHeight(canonical, out int height) ? height : null;
    }

    private static bool TryParseResolution(string widthText, string heightText, out int width, out int height)
    {
        width = 0;
        height = 0;
        if (!int.TryParse(widthText, out width) || !int.TryParse(heightText, out height))
            return false;
        return width is >= 16 and <= 16384 && height is >= 16 and <= 4320;
    }

    private void RememberDeliveredHeight(string url, int height)
    {
        if (string.IsNullOrWhiteSpace(url) || height <= 0) return;
        lock (_deliveredHeightLock)
        {
            _deliveredHeights[url] = (height, DateTime.UtcNow);
            PruneDeliveredHeightsLocked();
        }
    }

    private bool TryGetDeliveredHeight(string url, out int height)
    {
        height = 0;
        if (string.IsNullOrWhiteSpace(url)) return false;
        lock (_deliveredHeightLock)
        {
            if (!_deliveredHeights.TryGetValue(url, out var entry))
                return false;
            if (DateTime.UtcNow - entry.At > DeliveredHeightTtl)
            {
                _deliveredHeights.Remove(url);
                return false;
            }
            height = entry.Height;
            return true;
        }
    }

    private void PruneDeliveredHeightsLocked()
    {
        var now = DateTime.UtcNow;
        foreach (var key in _deliveredHeights
            .Where(kvp => now - kvp.Value.At > DeliveredHeightTtl)
            .Select(kvp => kvp.Key)
            .ToArray())
        {
            _deliveredHeights.Remove(key);
        }

        while (_deliveredHeights.Count > MaxDeliveredHeightEntries)
        {
            string? oldestKey = null;
            DateTime oldestAt = DateTime.MaxValue;
            foreach (var kvp in _deliveredHeights)
            {
                if (kvp.Value.At < oldestAt)
                {
                    oldestAt = kvp.Value.At;
                    oldestKey = kvp.Key;
                }
            }
            if (oldestKey == null) break;
            _deliveredHeights.Remove(oldestKey);
        }
    }

    private void StartPlayingFeedbackLoop(string url, DateTime openedAt)
    {
        if (string.IsNullOrWhiteSpace(url)) return;

        CancellationTokenSource? oldCts = null;
        var newCts = new CancellationTokenSource();
        lock (_stallLock)
        {
            if (string.Equals(_playingFeedbackUrl, url, StringComparison.Ordinal)
                && _playingFeedbackCts != null
                && !_playingFeedbackCts.IsCancellationRequested)
            {
                try { newCts.Dispose(); } catch { }
                return;
            }
            oldCts = _playingFeedbackCts;
            _playingFeedbackCts = newCts;
            _playingFeedbackUrl = url;
        }
        try { oldCts?.Cancel(); oldCts?.Dispose(); } catch { }

        _ = Task.Run(async () =>
        {
            while (!newCts.IsCancellationRequested)
            {
                try { await Task.Delay(PlayingFeedbackInterval, newCts.Token).ConfigureAwait(false); }
                catch (OperationCanceledException) { return; }
                catch (ObjectDisposedException) { return; }

                if (!TryGetDeliveredHeight(url, out int height))
                {
                    CancelPlayingFeedbackLoop(newCts);
                    return;
                }

                _ = _mesh.SendPlaybackFeedbackAsync(
                    url,
                    WireConstants.PlaybackFeedbackPlaying,
                    ElapsedMsSince(openedAt),
                    height);
            }
        });
    }

    private void CancelPlayingFeedbackLoop(CancellationTokenSource? expected = null)
    {
        CancellationTokenSource? cts;
        lock (_stallLock)
        {
            if (expected != null && !ReferenceEquals(_playingFeedbackCts, expected))
                return;
            cts = _playingFeedbackCts;
            _playingFeedbackCts = null;
            _playingFeedbackUrl = null;
        }
        try { cts?.Cancel(); cts?.Dispose(); } catch { }
    }

    private static int ElapsedMsSince(DateTime startedAt)
    {
        if (startedAt == default) return 0;
        double elapsed = (DateTime.UtcNow - startedAt).TotalMilliseconds;
        if (elapsed <= 0) return 0;
        return elapsed >= int.MaxValue ? int.MaxValue : (int)elapsed;
    }

    internal static string CanonicalPlaybackObservationUrl(string url)
    {
        return TrustGatewayUrlBuilder.TryExtractTarget(url, out string targetUrl)
            && FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUrl(targetUrl)
            ? targetUrl
            : url;
    }

    internal readonly record struct PlaybackFailureRecovery(int Evicted, bool OgHintArmed);

    private void CancelStallWatchdog()
    {
        CancellationTokenSource? cts;
        lock (_stallLock)
        {
            cts = _stallCts;
            _stallCts = null;
            _stallUrl = null;
        }
        try { cts?.Cancel(); cts?.Dispose(); } catch { }
    }

    public void Dispose()
    {
        _cts.Cancel();
        CancelStallWatchdog();
        CancelPlayingFeedbackLoop();
        _cts.Dispose();
    }
}
