using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed partial class LocalIpcServer : IDisposable
{
    private static readonly TimeSpan PerRequestTimeout = TimeSpan.FromSeconds(15);
    private const int WrapperBudgetFloorMs = 5_000;
    private const int WrapperBudgetCeilingMs = 90_000;
    private const int WrapperBudgetSafetyMarginMs = 500;
    private const int MaxRequestBytes = 4 * 1024 * 1024;

    private readonly MeshClient _mesh;
    private readonly ResolveCache? _cache;
    private readonly OgFallbackHint? _ogFallbackHint;
    private readonly ResolverHealthGate? _health;
    private readonly CancellationTokenSource _cts = new();
    private Task? _accepter;
    private Task? _legacyAccepter;

    public LocalIpcServer(MeshClient mesh, ResolveCache? cache = null, OgFallbackHint? ogFallbackHint = null, ResolverHealthGate? health = null)
    {
        _mesh = mesh;
        _cache = cache;
        _ogFallbackHint = ogFallbackHint;
        _health = health;
    }

    public void Start()
    {
        _accepter = Task.Run(() => AcceptLoopAsync(WireConstants.PipeName, _cts.Token));
        _legacyAccepter = Task.Run(() => AcceptLoopAsync(LegacyCompat.LegacyPipeName, _cts.Token));
    }

    internal void StartForTests(string pipeName)
    {
        _accepter = Task.Run(() => AcceptLoopAsync(pipeName, _cts.Token));
    }

    public async Task StopAsync()
    {
        _cts.Cancel();
        if (_accepter != null)
        {
            try { await _accepter.ConfigureAwait(false); } catch { }
        }
        if (_legacyAccepter != null)
        {
            try { await _legacyAccepter.ConfigureAwait(false); } catch { }
        }
    }

    private async Task AcceptLoopAsync(string pipeName, CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            NamedPipeServerStream pipe;
            try
            {
                pipe = CreatePipeWithLowIntegrityLabel(pipeName);
            }
            catch (Exception ex)
            {
                ConsoleUx.Warn(LogComponent.Ipc, "could not create pipe instance: " + ex.Message);
                try { await Task.Delay(1000, ct).ConfigureAwait(false); } catch { return; }
                continue;
            }

            try
            {
                await pipe.WaitForConnectionAsync(ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                pipe.Dispose();
                return;
            }
            catch (Exception ex)
            {
                ConsoleUx.Warn(LogComponent.Ipc, "accept failed: " + ex.Message);
                pipe.Dispose();
                continue;
            }

            _ = Task.Run(() => HandleAsync(pipe, ct));
        }
    }

    private async Task HandleAsync(NamedPipeServerStream pipe, CancellationToken outerCt)
    {
        using var perReqCts = CancellationTokenSource.CreateLinkedTokenSource(outerCt);
        perReqCts.CancelAfter(PerRequestTimeout);
        var deadlineUtc = DateTime.UtcNow + PerRequestTimeout;
        string id = "";
        string? cid = null;
        var swReq = Stopwatch.StartNew();
        try
        {
            var (line, truncated) = await ReadLineAsync(pipe, perReqCts.Token).ConfigureAwait(false);
            if (truncated)
            {
                ConsoleUx.Warn(LogComponent.Ipc, "rejecting request: payload exceeded "
                    + MaxRequestBytes + " bytes without a newline terminator");
                await WriteFallbackAsync(pipe, id, WireConstants.FallbackInternalError, perReqCts.Token).ConfigureAwait(false);
                return;
            }

            if (!string.IsNullOrWhiteSpace(line) && LooksLikeWrapperEventNotify(line))
            {
                try
                {
                    var notify = JsonSerializer.Deserialize(line, MeshJsonContext.Default.WrapperEventNotify);
                    if (notify != null) HandleWrapperEvent(notify);
                }
                catch (Exception ex)
                {
                    Logger.WriteFileOnly("[wrapper][warn] wrapper event parse failed: "
                        + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 160));
                }
                return;
            }

            ResolveRequest? req = null;
            string? parseError = null;
            if (!string.IsNullOrWhiteSpace(line))
            {
                try { req = JsonSerializer.Deserialize(line, MeshJsonContext.Default.ResolveRequest); }
                catch (Exception ex) { parseError = ex.GetType().Name + ": " + ex.Message; }
            }

            if (req == null || string.IsNullOrEmpty(req.Url))
            {
                if (parseError != null)
                {
                    ConsoleUx.Warn(LogComponent.Ipc, "request parse failed: "
                        + LogUtil.SanitizeForConsole(parseError, 160)
                        + " preview=" + LogUtil.SanitizeForConsole(line, 80));
                }
                else if (req != null)
                {
                    ConsoleUx.Warn(LogComponent.Ipc, "request missing url");
                }
                else
                {
                    ConsoleUx.Warn(LogComponent.Ipc, "empty request received");
                }
                await WriteFallbackAsync(pipe, id, WireConstants.FallbackInternalError, perReqCts.Token).ConfigureAwait(false);
                return;
            }

            id = req.Id ?? "";
            cid = req.CorrelationId;

            if (!string.Equals(req.Action, WireConstants.ActionResolve, StringComparison.Ordinal))
            {
                ConsoleUx.Warn(LogComponent.Ipc, "rejecting request id=" + id +
                    " action=" + LogUtil.SanitizeForConsole(req.Action, 32) +
                    " -- only \"resolve\" is accepted on this pipe");
                await WriteFallbackAsync(pipe, id, WireConstants.FallbackInternalError, perReqCts.Token).ConfigureAwait(false);
                return;
            }

            if (req.Player != WireConstants.PlayerAvPro && req.Player != WireConstants.PlayerUnity)
            {
                ConsoleUx.Warn(LogComponent.Ipc, "rejecting request id=" + id + CidSuffix(cid) +
                    " player=" + LogUtil.SanitizeForConsole(req.Player ?? "<null>", 32) +
                    " -- must be \"avpro\" or \"unity\" (case-sensitive)");
                await WriteFallbackAsync(pipe, id, WireConstants.FallbackInternalError, perReqCts.Token).ConfigureAwait(false);
                return;
            }

            if (req.WrapperDeadlineMs is int wrapperBudgetMs && wrapperBudgetMs > 0)
            {
                int effectiveMs = Math.Clamp(
                    wrapperBudgetMs - WrapperBudgetSafetyMarginMs,
                    WrapperBudgetFloorMs,
                    WrapperBudgetCeilingMs);
                perReqCts.CancelAfter(TimeSpan.FromMilliseconds(effectiveMs));
                deadlineUtc = DateTime.UtcNow.AddMilliseconds(effectiveMs);
                Logger.WriteFileOnly("[ipc] honoring wrapper_deadline_ms id=" + id +
                    " wrapper_deadline_ms=" + wrapperBudgetMs +
                    " effective_ms=" + effectiveMs);
            }

            if (req.Player == WireConstants.PlayerAvPro && req.AcceptCodecs != null)
                req.AcceptCodecs = WireConstants.BuildAcceptCodecs(CodecCapabilityProbe.VerifiedVideoCodecs);

            if (ResolveRequestProfile.ApplyHighQuality(req, AppSettingsStore.Shared.Snapshot().Playback.HighQuality))
                Logger.WriteFileOnly("[ipc] high quality on; requesting up to "
                    + WireConstants.HighQualityMaxHeight + "p id=" + id);

            string host = LogUtil.BareHost(req.Url);
            bool viaLhYt = IsLocalhostYoutubeUrl(req.Url);
            string playerLabel = FormatPlayerLabel(req);
            WatchdogStats.RecordResolve(viaLhYt);

            string? failReason = null;
            string outcome = "?";
            string? serverReason = null;
            bool viaCache = false;
            string nodeHost = _mesh.CurrentNodeHost;

            if (_ogFallbackHint != null && _ogFallbackHint.ShouldPreferOg(req.Url))
            {
                await WriteFallbackAsync(pipe, id,
                    WireConstants.OgFallbackReasonPriorLoadFailure,
                    perReqCts.Token).ConfigureAwait(false);
                ConsoleUx.Write(LogComponent.Ipc,
                    "og-fallback (prior load_failure) id=" + id
                        + CidSuffix(cid)
                        + " host=" + LogUtil.BareHost(req.Url));
                return;
            }

            if (_health != null)
            {
                bool paused = _health.ShouldShortCircuit(_mesh.IsConnected, out var gateCheck);
                if (gateCheck == ResolverHealthGate.Transition.Closed)
                    ConsoleUx.Success(LogComponent.Ipc, "resolver restored -- mesh resolving re-enabled");
                if (paused)
                {
                    await WriteFallbackAsync(pipe, id,
                        WireConstants.OgFallbackReasonResolverUnhealthy,
                        perReqCts.Token).ConfigureAwait(false);
                    ConsoleUx.Write(LogComponent.Ipc,
                        "og-fallback (resolver paused) id=" + id
                            + CidSuffix(cid)
                            + " host=" + LogUtil.BareHost(req.Url));
                    return;
                }
            }

            try
            {
                CachedResolve? cached = _cache?.Lookup(nodeHost, req.Url, req.Player, req.VrchatFormatArg, req.MaxHeight, req.Id ?? "");
                if (cached.HasValue)
                {
                    await WriteFrameAsync(pipe, cached.Value.Frame, perReqCts.Token).ConfigureAwait(false);
                    outcome = cached.Value.Action;
                    serverReason = cached.Value.Reason;
                    viaCache = true;
                    WatchdogStats.RecordCacheHit();
                    Logger.WriteFileOnly("[resolve-cache] hit id=" + id +
                        " host=" + LogUtil.BareHost(req.Url) +
                        " bytes=" + cached.Value.Frame.Length);
                    _ = _mesh.SendPlaybackFeedbackAsync(req.Url,
                        WireConstants.PlaybackFeedbackCachePlay, 0,
                        correlationIdOverride: string.IsNullOrEmpty(cid) ? id : cid);
                }
                else
                {
                    if (string.IsNullOrEmpty(req.CorrelationId) && MeshClient.CallerOptedIntoV2(req))
                    {
                        req.CorrelationId = req.Id;
                        cid = req.CorrelationId;
                    }

                    MeshResolveResult result = await _mesh.ResolveAsync(req, perReqCts.Token).ConfigureAwait(false);

                    int retriesSent = 0;
                    while (result.Action == WireConstants.ActionFallbackNative
                        && ResolveRetryPolicy.ShouldRetry(result.Reason, retriesSent,
                            (long)(deadlineUtc - DateTime.UtcNow).TotalMilliseconds)
                        && !perReqCts.Token.IsCancellationRequested)
                    {
                        int delayMs = ResolveRetryPolicy.NextDelayMs(retriesSent);
                        Logger.WriteFileOnly("[ipc] retry id=" + id + CidSuffix(cid)
                            + " reason=" + LogUtil.SanitizeForConsole(result.Reason ?? "?", 32)
                            + " attempt=" + (retriesSent + 2)
                            + " delay_ms=" + delayMs);
                        try
                        {
                            await Task.Delay(delayMs, perReqCts.Token).ConfigureAwait(false);
                        }
                        catch (OperationCanceledException) { break; }
                        if (perReqCts.Token.IsCancellationRequested) break;

                        req.Id = Guid.NewGuid().ToString("N");
                        retriesSent++;
                        result = await _mesh.ResolveAsync(req, perReqCts.Token).ConfigureAwait(false);
                    }

                    await WriteFrameAsync(pipe, result.Frame, perReqCts.Token).ConfigureAwait(false);
                    outcome = result.Action;
                    serverReason = result.Reason;

                    if (_cache != null && outcome == WireConstants.ActionResolved && !string.IsNullOrEmpty(nodeHost))
                    {
                        try
                        {
                            var parsed = JsonSerializer.Deserialize(result.Frame, MeshJsonContext.Default.ResolveResponse);
                            if (parsed != null)
                            {
                                bool defaultTtl = string.IsNullOrEmpty(parsed.ExpiresAt);
                                _cache.Store(nodeHost, req.Url, req.Player, req.VrchatFormatArg, req.MaxHeight, parsed);
                                Logger.WriteFileOnly("[resolve-cache] stored id=" + id +
                                    " host=" + LogUtil.BareHost(req.Url) +
                                    (defaultTtl ? " ttl=default" : " ttl=server"));
                            }
                        }
                        catch (Exception ex)
                        {
                            Logger.WriteFileOnly("[resolve-cache] store failed id=" + id +
                                ": " + ex.GetType().Name + ": " + ex.Message);
                        }
                    }
                }
            }
            catch (OperationCanceledException)
            {
                failReason = WireConstants.FallbackServerUnreachable;
            }
            catch (Exception ex)
            {
                ConsoleUx.Warn(
                    LogComponent.Ipc,
                    "mesh.ResolveAsync threw id=" + id + CidSuffix(cid) +
                    ": " + ex.GetType().Name + ": " +
                    LogUtil.SanitizeForConsole(ex.Message, 160));
                failReason = WireConstants.FallbackInternalError;
            }

            if (failReason != null)
            {
                outcome = WireConstants.ActionFallbackNative + "/" + failReason;
                using var writeCts = new CancellationTokenSource(TimeSpan.FromSeconds(2));
                await WriteFallbackAsync(pipe, id, failReason, writeCts.Token).ConfigureAwait(false);
                ReportingService.ReportFallback(req, failReason, null);
            }
            else if (outcome.StartsWith(WireConstants.ActionFallbackNative))
            {
                string reason = outcome.Length > WireConstants.ActionFallbackNative.Length + 1
                    ? outcome[(WireConstants.ActionFallbackNative.Length + 1)..]
                    : "";
                if (!string.IsNullOrEmpty(reason))
                    ReportingService.ReportFallback(req, reason, null);
            }

            swReq.Stop();
            ResolveStatus status;
            string? reasonForLine = null;
            if (outcome == WireConstants.ActionResolved)
            {
                status = ResolveStatus.Resolved;
            }
            else if (failReason != null)
            {
                status = ResolveStatus.Failed;
                reasonForLine = failReason;
            }
            else if (outcome == WireConstants.ActionFallbackNative)
            {
                status = ResolveStatus.Fallback;
                reasonForLine = !string.IsNullOrEmpty(serverReason) ? serverReason : "unspecified";
            }
            else
            {
                status = ResolveStatus.Unexpected;
                reasonForLine = outcome;
            }
            ConsoleUx.ResolveOutcome(
                host: host,
                player: playerLabel,
                status: status,
                viaCache: viaCache,
                viaLhYt: viaLhYt,
                elapsed: swReq.Elapsed,
                reason: reasonForLine);

            if (_health != null && !viaCache)
            {
                bool healthy = IsHealthyOutcome(failReason, outcome, serverReason);
                var gateShift = _health.RecordResolveOutcome(healthy, outcome == WireConstants.ActionResolved);
                if (gateShift == ResolverHealthGate.Transition.Opened)
                    ConsoleUx.Warn(LogComponent.Ipc,
                        "resolver paused -- " + ResolverHealthGate.OpenThreshold
                        + " resolves failed in a row; VRChat's own resolver takes over while the connection recovers");
                else if (gateShift == ResolverHealthGate.Transition.Closed)
                    ConsoleUx.Success(LogComponent.Ipc, "resolver restored -- mesh resolving re-enabled");
            }

            Logger.WriteFileOnly(
                "[ipc] resolve_dispatch_complete id=" + id + CidSuffix(cid) +
                " action=" + LogUtil.SanitizeForConsole(outcome, 48) +
                " reason=" + LogUtil.SanitizeForConsole(serverReason ?? failReason ?? "", 48) +
                " player=" + LogUtil.SanitizeForConsole(req.Player ?? WireConstants.PlayerUnknown, 16) +
                (viaCache ? " via=cache" : "") +
                " elapsed_ms=" + (long)swReq.Elapsed.TotalMilliseconds);
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(
                LogComponent.Ipc,
                "connection error id=" + id + CidSuffix(cid) +
                ": " + ex.GetType().Name + ": " +
                LogUtil.SanitizeForConsole(ex.Message, 160));
        }
        finally
        {
            try { if (pipe.IsConnected) pipe.Disconnect(); } catch { }
            pipe.Dispose();
        }
    }

    internal static bool IsHealthyOutcome(string? failReason, string outcome, string? serverReason)
        => failReason == null
           && !(outcome == WireConstants.ActionFallbackNative
               && (serverReason == WireConstants.FallbackServerUnreachable
                   || serverReason == WireConstants.FallbackInternalError));

    public void Dispose()
    {
        _cts.Cancel();
        _cts.Dispose();
    }
}
