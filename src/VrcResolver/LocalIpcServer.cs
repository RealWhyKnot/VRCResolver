using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using VrcResolver.Shared;

namespace VrcResolver;

// Named-pipe server at \\.\pipe\vrcresolver.resolve. The patched yt-dlp.exe
// connects, sends one ResolveRequest, reads one ResolveResponse, and closes.
// A second accept loop serves the pre-rename pipe name (LegacyCompat) with
// the same handler: an old wrapper still sitting in VRChat's Tools dir keeps
// dialing the old name until PatchManager swaps it for the new build.
//
// ACL: pipe is created with an explicit security descriptor that grants the
// current user full access (DACL) AND tags the pipe with a Low-integrity
// mandatory label (SACL `S:(ML;;NW;;;LW)`). Without the Low-integrity SACL,
// the wrapper deployed into VRChat's Tools dir (Low-integrity, inherited
// from the LocalLow path) can't connect — Windows MIC blocks the connect
// attempt before the DACL check fires. This was a silent bug for an entire
// session: VRChat invoked our wrapper, wrapper's pipe connect failed, wrapper
// silently fell through to og fallback. Mesh path bypassed entirely.
//
// Wire format on the pipe is newline-delimited JSON: client writes one
// request followed by '\n', server writes one response followed by '\n'.
// Newline framing keeps both sides simple — no length prefixes, no
// read-to-end hangs that would happen with raw stream deserialization.
//
// Per-connection budget defaults to 15 s; a wrapper-declared
// wrapper_deadline_ms overrides it (clamped, minus a safety margin). On
// timeout/parse-error/MeshClient throwing we synthesize a fallback_native
// frame with the appropriate reason rather than dropping the connection, so
// the patched yt-dlp.exe always gets a definitive answer it can act on.
[SupportedOSPlatform("windows")]
internal sealed partial class LocalIpcServer : IDisposable
{
    // Default per-request budget when the wrapper does not declare its
    // own deadline (old clients, manual JSON over the pipe, etc.). The
    // wrapper now sends `wrapper_deadline_ms` on every resolve and the
    // watchdog overrides this default so the mesh-side wait aligns with
    // however long the wrapper is actually willing to wait, minus a
    // 500 ms safety margin so the synthesized fallback_native still
    // wins the race if the timeout fires.
    private static readonly TimeSpan PerRequestTimeout = TimeSpan.FromSeconds(15);
    // Hard floor + ceiling on the per-request budget when honoring a
    // wrapper-declared deadline. The floor prevents a zero / tiny budget
    // from making every resolve insta-fail. The ceiling caps trust in a
    // misbehaving wrapper to a value still well below the WS keepalive.
    private const int WrapperBudgetFloorMs = 5_000;
    private const int WrapperBudgetCeilingMs = 90_000;
    private const int WrapperBudgetSafetyMarginMs = 500;
    // Match the WS-side 4 MiB cap so a giant vrchat_format_arg (raw yt-dlp
    // -f selector) round-trips end-to-end. Pre-fix this was 64 KiB which
    // silently truncated large selectors mid-string; the resulting
    // truncated JSON failed to parse and surfaced as fallback_internal_error
    // with no diagnostic about WHY.
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
        // Transition-window listener on the pre-rename pipe name. Old
        // wrappers dial this until PatchManager swaps them; both loops
        // delegate to the same handler.
        _legacyAccepter = Task.Run(() => AcceptLoopAsync(LegacyCompat.LegacyPipeName, _cts.Token));
    }

    // Single accept loop on a caller-chosen pipe name so tests can exercise
    // the real request handling end to end without claiming the global names.
    internal void StartForTests(string pipeName)
    {
        _accepter = Task.Run(() => AcceptLoopAsync(pipeName, _cts.Token));
    }

    public async Task StopAsync()
    {
        _cts.Cancel();
        if (_accepter != null)
        {
            try { await _accepter.ConfigureAwait(false); } catch { /* ignore */ }
        }
        if (_legacyAccepter != null)
        {
            try { await _legacyAccepter.ConfigureAwait(false); } catch { /* ignore */ }
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

            // v3.2: peek the action field FIRST. If it's one of the
            // wrapper's notification frames (og_fallback_notify or
            // wrapper_og_failed), dispatch separately -- the wire shape
            // is a different DTO (WrapperEventNotify) and the wrapper
            // closes the pipe immediately after writing without waiting
            // for a response.
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
                // Surface parse failures + missing-url cases so a misbehaving
                // patched yt-dlp is diagnosable from the watchdog console.
                // Pre-fix this path was completely silent.
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

            // H12: validate action vocabulary. The DTO accepts any string;
            // a request with action="ping" or any non-resolve verb that
            // happens to also carry a url would otherwise be silently
            // forwarded to the mesh (which would reject — but with no
            // diagnostic on the watchdog side).
            if (!string.Equals(req.Action, WireConstants.ActionResolve, StringComparison.Ordinal))
            {
                ConsoleUx.Warn(LogComponent.Ipc, "rejecting request id=" + id +
                    " action=" + LogUtil.SanitizeForConsole(req.Action, 32) +
                    " -- only \"resolve\" is accepted on this pipe");
                await WriteFallbackAsync(pipe, id, WireConstants.FallbackInternalError, perReqCts.Token).ConfigureAwait(false);
                return;
            }

            // H11: validate player vocabulary. Server spec is case-sensitive
            // "avpro" | "unity"; anything else (including null/empty,
            // "AVPro", "AvPro") gets rejected here with a clear log line so
            // patched-yt-dlp casing drift surfaces in a bug report instead
            // of silently being routed to a server that will reject.
            if (req.Player != WireConstants.PlayerAvPro && req.Player != WireConstants.PlayerUnity)
            {
                ConsoleUx.Warn(LogComponent.Ipc, "rejecting request id=" + id + CidSuffix(cid) +
                    " player=" + LogUtil.SanitizeForConsole(req.Player ?? "<null>", 32) +
                    " -- must be \"avpro\" or \"unity\" (case-sensitive)");
                await WriteFallbackAsync(pipe, id, WireConstants.FallbackInternalError, perReqCts.Token).ConfigureAwait(false);
                return;
            }

            // If the wrapper declared its own deadline, align the watchdog's
            // per-request budget with it (minus a 500 ms safety margin so the
            // synthesized fallback_native lands before the wrapper gives up).
            // Old wrappers that omit the field keep the PerRequestTimeout
            // default armed at the top of HandleAsync. Floor and ceiling
            // bound the trust placed in a misbehaving wrapper.
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

            // Verified codec claims: replace the wrapper's static AVPro
            // accept_codecs with the capability-probe-filtered list, so the
            // server only ever sees codecs this machine can decode. The
            // request stays a multi-entry set (h264/aac baseline + audio
            // codecs always survive); only unverified extension-backed video
            // codecs are pruned. v1 callers that omitted the field keep
            // their bytes untouched.
            if (req.Player == WireConstants.PlayerAvPro && req.AcceptCodecs != null)
                req.AcceptCodecs = WireConstants.BuildAcceptCodecs(CodecCapabilityProbe.VerifiedVideoCodecs);

            // Capture the host + player labels for the single per-resolve
            // summary line that fires at terminal-response time below.
            // The earlier two-line layout (cyan request line at arrival
            // + colored response line at terminus) was traded for one
            // line per resolve so busy worlds don't double-scroll.
            //
            // `[via lh-yt]` fires when the user-pasted URL host is
            // localhost.youtube.com -- the public-instance trust-list
            // bypass path. Surfaces at-a-glance whether the
            // public-world workaround is being exercised. Same
            // per-process counter goes to the heartbeat line for
            // aggregate visibility.
            string host = LogUtil.BareHost(req.Url);
            bool viaLhYt = IsLocalhostYoutubeUrl(req.Url);
            string playerLabel = FormatPlayerLabel(req);
            WatchdogStats.RecordResolve(viaLhYt);

            string? failReason = null;
            string outcome = "?";
            string? serverReason = null;
            bool viaCache = false;
            string nodeHost = _mesh.CurrentNodeHost;

            // Reactive og-fallback: an AVPro load_failure for this source
            // within the last ~60 s short-circuits the entire mesh path so
            // the wrapper execs yt-dlp-og.exe immediately. Set by
            // VrcLogMonitor on the failing playback's resolved URL and
            // unwound by TTL. Cache-hit path is intentionally bypassed too:
            // the cache entry is likely the one that produced the failure,
            // and even if VrcLogMonitor already evicted it the goal is to
            // give VRChat a known-good native URL on the next retry.
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

            // Health gate: repeated resolve or playback failures pause the
            // whole mesh path -- cache included, since a mesh-down node may
            // not serve the bytes behind a cached proxy URL either. The
            // wrapper sees a non-retryable reason and execs og after a
            // single pipe roundtrip. The first request after cooldown (with
            // the socket actually open) passes through as the probe.
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
                // v3.2: resolve disk-cache lookup. If we have a cached
                // `resolved` frame for (nodeHost, url, player, format)
                // whose server-issued expires_at is still > now + 30s,
                // replay it directly to the wrapper -- skip the WS
                // round-trip + server-side lookup. Cache cap = 500
                // entries; staleness is closed via VrcLogMonitor's
                // silent_stall hook calling EvictByUrl.
                CachedResolve? cached = _cache?.Lookup(nodeHost, req.Url, req.Player, req.VrchatFormatArg, req.Id ?? "");
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
                    // Feature playback_feedback_v2: tell the server a cached
                    // resolve produced a play it never saw, so per-domain play
                    // counts stop undercounting by the cache-hit rate.
                    // Fire-and-forget; MeshClient drops it on older servers.
                    _ = _mesh.SendPlaybackFeedbackAsync(req.Url,
                        WireConstants.PlaybackFeedbackCachePlay, 0,
                        correlationIdOverride: string.IsNullOrEmpty(cid) ? id : cid);
                }
                else
                {
                    // Lossless forward: hand the whole DTO to MeshClient so v2 fields
                    // (protocol_version / accept_protocols / accept_codecs / etc.)
                    // and any unknown fields populated by the patched yt-dlp pass
                    // through to the mesh server unchanged. The DTO's
                    // [JsonExtensionData] bag preserves anything we don't statically
                    // know about.
                    //
                    // ResolveAsync returns the verified raw response bytes plus
                    // the pre-extracted action and server-supplied reason. We
                    // write the bytes straight to the pipe -- no JsonDocument
                    // re-encode on the hot path -- and use the extracted strings
                    // for the user-facing console summary.
                    // The watchdog is the ONE retry owner (the wrapper only
                    // retries a failed pipe connect): it alone sees mesh
                    // connection state, the rate-limit cooldown, and the
                    // health gate. Fresh id per attempt with correlation_id
                    // stamped once is the server's documented contract --
                    // per-attempt ids join on correlation_id in its logs and
                    // its dedup is per-domain, not per-id. The correlation
                    // stamp is gated on v2 opt-in so a strict-shape v1
                    // caller's bytes stay v1 end to end.
                    if (string.IsNullOrEmpty(req.CorrelationId) && MeshClient.CallerOptedIntoV2(req))
                    {
                        req.CorrelationId = req.Id;
                        cid = req.CorrelationId;
                    }

                    MeshResolveResult result = await _mesh.ResolveAsync(req, perReqCts.Token).ConfigureAwait(false);

                    // Retry transient reasons on the shared policy (the ONE
                    // reason list + schedule). Structural reasons won't
                    // change on retry; the wrapper falls back to og after a
                    // single pipe roundtrip.
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

                    // Cache the response on terminal `resolved` with a
                    // non-null expires_at. ResolveCache.Store gates these
                    // conditions itself; we still parse the frame here so
                    // the typed ResolveResponse round-trips cleanly through
                    // the source-gen path on subsequent hits.
                    if (_cache != null && outcome == WireConstants.ActionResolved && !string.IsNullOrEmpty(nodeHost))
                    {
                        try
                        {
                            var parsed = JsonSerializer.Deserialize(result.Frame, MeshJsonContext.Default.ResolveResponse);
                            if (parsed != null)
                            {
                                bool defaultTtl = string.IsNullOrEmpty(parsed.ExpiresAt);
                                _cache.Store(nodeHost, req.Url, req.Player, req.VrchatFormatArg, parsed);
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
                // Deliberately NOT perReqCts (already fired on this path),
                // but bounded: an unbounded write to a wrapper that stopped
                // reading parked this handler task forever.
                using var writeCts = new CancellationTokenSource(TimeSpan.FromSeconds(2));
                await WriteFallbackAsync(pipe, id, failReason, writeCts.Token).ConfigureAwait(false);
                ReportingService.ReportFallback(req, failReason, null);
            }
            else if (outcome.StartsWith(WireConstants.ActionFallbackNative))
            {
                // Mesh returned a fallback_native frame. Reach into the
                // dispatched response for the reason code; ReportingService
                // filters out transient kinds itself.
                string reason = outcome.Length > WireConstants.ActionFallbackNative.Length + 1
                    ? outcome[(WireConstants.ActionFallbackNative.Length + 1)..]
                    : "";
                if (!string.IsNullOrEmpty(reason))
                    ReportingService.ReportFallback(req, reason, null);
            }

            // User-facing per-resolve summary -- single line per resolve.
            // Format:
            //   <host> [via lh-yt] (<player>)  <status>  <elapsed>
            // Colour signals at-a-glance status: green = resolved (mesh
            // or cached), yellow = server replied with fallback_native
            // (og takes over), red = we synthesised fallback_native
            // locally (server timeout / IPC budget tripped), gray =
            // unexpected outcome. Standing rule from
            // feedback_no_console_spam.md: one summary line per resolve,
            // not a START + END pair.
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

            // Feed the health gate. Cache hits never touched the mesh, so
            // they are no evidence either way. healthy = a real server
            // verdict; synthesized unreachable/internal outcomes are what
            // the resolve streak counts.
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

            // Detailed per-request line (id, cid, full outcome) routed to
            // the rolling watchdog log only -- kept off the user-facing
            // console window so the friendly summary above stays scannable.
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
            try { if (pipe.IsConnected) pipe.Disconnect(); } catch { /* ignore */ }
            pipe.Dispose();
        }
    }

    // healthy = a real server verdict, of any kind. Only synthesized
    // unreachable/internal outcomes count against the resolve streak; a server
    // that answers rate_limited / protocol_error / domain-shaped failures is
    // alive and must not open the gate.
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
