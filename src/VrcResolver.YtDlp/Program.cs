using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

// VRChat invokes its `Tools\yt-dlp.exe` for every video. PatchManager swaps
// VRChat's bundled vanilla yt-dlp for THIS binary at watchdog startup; the
// vanilla copy is preserved as `Tools\yt-dlp-og.exe` so we have an
// unconditional fallback target.
//
// Behaviour per invocation:
//   1. Extract the URL (first http(s) arg) and the `-f <selector>` value
//      from VRChat's argv. Infer player from `-f` (Unity caps height<=720;
//      anything else defaults to AVPro).
//   2. Connect to `\\.\pipe\vrcresolver.resolve` with a 1 s connect budget.
//   3. Send a v2 ResolveRequest as a single NDJSON line, including
//      vrchat_format_arg (raw -f value), accept_protocols / accept_codecs
//      defaults per the inferred player, and protocol_version=2.
//   4. Read one response line (28 s ceiling — must outlast the watchdog's
//      LocalIpcServer per-request timeout so the synthesized fallback
//      response reliably wins the race).
//      - action=resolved + url present  → write URL to stdout, exit 0.
//      - action=fallback_native (any reason) → exec sibling yt-dlp-og.exe
//        with the original argv, pass through stdout/stderr/exit-code.
//      - any failure (no pipe, parse error, IO error, timeout, no URL field)
//        → same exec.
//
// Graceful-degradation contract: when the watchdog isn't running, when the
// server returns fallback_native, and when yt-dlp-og.exe is unavailable or
// crashes — VRChat must see the SAME behaviour it would see if vrcresolver
// weren't installed at all. yt-dlp-og.exe is the preserved vanilla copy;
// its stdout/stderr/exit-code is passed through unmodified. When even
// yt-dlp-og.exe is missing, we emit empty stdout + exit 0 so VRChat's
// resolver sees "no URL found" and falls into its own error path — never
// the original URL re-emitted as if we'd resolved to it.
//
// Stdout contract: VRChat's bundled yt-dlp writes the resolved URL on a
// single line terminated by exactly one '\n' — no CRLF, no BOM. We match
// that via raw Console.OpenStandardOutput().
//
// Logging: every invocation appends a start banner, a step-by-step trace,
// a fallback-stdout/stderr preview, and an END summary line to
// %LOCALAPPDATA%Low\vrcresolver\logs\yt-dlp-wrapper.log. Each line is prefixed
// with [<utc>] [<rid>] for grep correlation. URLs in argv are sanitized to
// host-only when logged (no path/query — they may carry tokens).
[SupportedOSPlatform("windows")]
internal static partial class Program
{
    private static readonly TimeSpan PipeConnectTimeout = TimeSpan.FromSeconds(1);
    // Overall budget for a complete response. The watchdog now sizes its own
    // per-request timeout off wrapper_deadline_ms (LocalIpcServer.cs), so this
    // single value drives the end-to-end budget. Cold extractors on slow sites
    // (Tubi has been observed at ~20-25 s) need the full window; setting this
    // shorter than the cold-discovery worst case is what forced the historical
    // "paste the URL twice" behaviour.
    private static readonly TimeSpan ResolveDeadline = TimeSpan.FromSeconds(28);

    // Per-invocation correlation id. 8 hex chars — enough to tell two
    // overlapping invocations apart when VRChat fires retries quickly.
    private static string s_rid = "????????";

    private static async Task<int> Main(string[] args)
    {
        // No Console.OutputEncoding setup here — the wrapper writes its
        // resolved URL via raw Console.OpenStandardOutput().Write(bytes)
        // and og fallback via the same raw write. Console.WriteLine is
        // never called. Setting OutputEncoding triggers SetConsoleOutputCP
        // syscalls + buffer flushing that just waste 1-3 ms per invocation.
        s_rid = Guid.NewGuid().ToString("N")[..8];
        var swTotal = Stopwatch.StartNew();

        try
        {
            string url = ExtractUrl(args);
            string? formatArg = ExtractDashFValue(args);
            string player = ResolveRequestProfile.InferPlayer(formatArg);

            LogStartBanner(args, url, formatArg, player);

            int exitCode;
            string outcome;

            if (string.IsNullOrEmpty(url))
            {
                // No URL in argv -- not a resolve invocation (e.g.,
                // `yt-dlp --version`, `--help`, or a probe). Forward
                // straight to vanilla so diagnostic invocations work.
                Log("no URL in argv -- exec og fallback (diagnostic invocation)");
                await TrySendOgFallbackNotifyAsync(null, WireConstants.OgFallbackReasonNoUrlDiagnostic, swTotal.ElapsedMilliseconds).ConfigureAwait(false);
                exitCode = await ExecFallbackAsync(args, null).ConfigureAwait(false);
                outcome = "no-url-fallback";
            }
            else
            {
                if (FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUrl(url))
                {
                    // Users can paste a first-party playback URL directly into
                    // a VRChat player. Do not send that back through the mesh
                    // resolver: it is already a server-minted playback URL,
                    // and resolving it again can recurse through /api/proxy.
                    string toEmit = TryWrapForTrustGateway(url, probeRelay: true);
                    TryWriteUrlToStdout(toEmit);
                    bool wrapped = !string.Equals(toEmit, url, StringComparison.Ordinal);
                    Log("direct first-party playback URL host=" + ExtractHost(url)
                        + " emitted-host=" + ExtractHost(toEmit)
                        + " bytes=" + toEmit.Length
                        + " trust_gateway=" + (wrapped ? "wrapped" : "passthrough"));
                    exitCode = 0;
                    outcome = wrapped ? "direct-first-party-playback-wrapped" : "direct-first-party-playback";
                }
                else
                {
                    (string? resolved, string? fallbackReason, string? publicMessage) result;
                    try
                    {
                        result = await ResolveOverPipeAsync(url, player, formatArg).ConfigureAwait(false);
                    }
                    catch (Exception ex)
                    {
                        Log("UNHANDLED in pipe path: " + ex.GetType().Name + ": " + ex.Message);
                        result = (null, WireConstants.OgFallbackReasonPipeResolveFailed, null);
                    }

                    // Emit-shape guard: the resolved URL crossed a pipe and,
                    // before that, a WebSocket and possibly a disk cache --
                    // none of which prove it is something VRChat should fetch.
                    // Loopback / private-range / non-http shapes go to og.
                    if (!string.IsNullOrEmpty(result.resolved)
                        && !ResolvedUrlGuard.IsSafeToEmit(result.resolved))
                    {
                        Log("resolved URL fails emit guard (host=" + ExtractHost(result.resolved!) + ") -- falling back");
                        result = (null, WireConstants.OgFallbackReasonResolvedUrlRejected, null);
                    }

                    if (!string.IsNullOrEmpty(result.resolved))
                    {
                        // Trust-gateway wrap (Phase 1, HTTP-only). If the
                        // watchdog is running and bound the local listener
                        // it leaves a port file on disk; first-party playback proxy
                        // URLs are wrapped through localhost.youtube.com:<port>/play
                        // so AVPro's allowlist accepts them in default-public
                        // worlds. Missing port file, non-first-party responses, or
                        // malformed URLs fall through to emitting the raw URL.
                        string toEmit = TryWrapForTrustGateway(result.resolved!);
                        TryWriteUrlToStdout(toEmit);
                        bool wrapped = !ReferenceEquals(toEmit, result.resolved);
                        Log("emitted resolved URL to stdout host=" + ExtractHost(toEmit)
                            + " bytes=" + toEmit.Length
                            + " trust_gateway=" + (wrapped ? "wrapped" : "passthrough"));
                        exitCode = 0;
                        outcome = wrapped ? "pipe-resolved-wrapped" : "pipe-resolved";
                    }
                    else
                    {
                        // Notify the watchdog about the og fallback before
                        // invoking og so the watchdog console surfaces the
                        // fallback fact in real time. Fire-and-forget; the
                        // notify uses a fresh pipe connection that closes
                        // immediately, then we exec og normally.
                        string reason = result.fallbackReason ?? WireConstants.OgFallbackReasonPipeResolveFailed;
                        await TrySendOgFallbackNotifyAsync(url, reason, swTotal.ElapsedMilliseconds, result.publicMessage).ConfigureAwait(false);

                        exitCode = await ExecFallbackAsync(args, url).ConfigureAwait(false);
                        outcome = "pipe-failed-og-fallback";
                    }
                }
            }

            swTotal.Stop();
            Log("END exit=" + exitCode + " outcome=" + outcome + " elapsed_ms=" + swTotal.ElapsedMilliseconds);
            return exitCode;
        }
        finally
        {
            CloseLog();
        }
    }

    // Returns (resolvedUrl, null) on success, (null, reason) on any
    // failure (caller falls back to yt-dlp-og.exe and uses `reason` to
    // notify the watchdog). Every exit branch logs.
    private static async Task<(string? Url, string? FallbackReason, string? PublicMessage)> ResolveOverPipeAsync(string url, string player, string? formatArg)
    {
        var swPipe = Stopwatch.StartNew();
        long totalDeadlineMs = (long)ResolveDeadline.TotalMilliseconds;
        int? maxHeight = ResolveRequestProfile.TryGetHeightCap(formatArg);

        var req = new ResolveRequest
        {
            Action = WireConstants.ActionResolve,
            Id = Guid.NewGuid().ToString("N"),
            Url = url,
            Player = player,
            MaxHeight = maxHeight,
            ProtocolVersion = WireConstants.ClientProtocolVersion,
            VrchatFormatArg = formatArg,
            AcceptProtocols = player == WireConstants.PlayerUnity
                ? WireConstants.UnityAcceptProtocols
                : WireConstants.AvProAcceptProtocols,
            AcceptCodecs = player == WireConstants.PlayerUnity
                ? WireConstants.UnityAcceptCodecs
                : WireConstants.AvProAcceptCodecs,
            MaxAudioChannels = player == WireConstants.PlayerUnity
                ? WireConstants.UnityMaxAudioChannels
                : WireConstants.AvProMaxAudioChannels,
        };

        // Retry loop: on discovery_in_progress or server_unreachable the
        // wrapper retries up to ResolveRetryPolicy.MaxRetries additional times
        // before falling back to og. The named pipe is re-established per
        // attempt because the watchdog closes its end after writing the
        // terminal frame; the request id stays the same across retries so the
        // server can deduplicate or correlate logs across attempts.
        var swRequest = Stopwatch.StartNew();
        int retriesSent = 0;
        while (true)
        {
            using var ctsConnect = new CancellationTokenSource(PipeConnectTimeout);
            using var pipe = new NamedPipeClientStream(
                ".",
                WireConstants.PipeName,
                PipeDirection.InOut,
                PipeOptions.Asynchronous);
            try
            {
                await pipe.ConnectAsync(ctsConnect.Token).ConfigureAwait(false);
                if (retriesSent == 0)
                    Log("pipe connect OK elapsed_ms=" + swPipe.ElapsedMilliseconds);
                else
                    Log("pipe reconnect OK retry=" + retriesSent + " elapsed_ms=" + swPipe.ElapsedMilliseconds);
            }
            catch (OperationCanceledException)
            {
                Log("pipe connect TIMED OUT after " + swPipe.ElapsedMilliseconds + " ms (watchdog not running?)");
                return (null, WireConstants.OgFallbackReasonPipeConnectFailed, null);
            }
            catch (System.IO.FileNotFoundException)
            {
                Log("pipe connect ENOENT (watchdog not running)");
                return (null, WireConstants.OgFallbackReasonPipeConnectFailed, null);
            }
            catch (Exception ex)
            {
                Log("pipe connect failed: " + ex.GetType().Name + ": " + ex.Message);
                return (null, WireConstants.OgFallbackReasonPipeConnectFailed, null);
            }

            long remainingForAttempt = totalDeadlineMs - swPipe.ElapsedMilliseconds;
            if (remainingForAttempt <= 0)
            {
                Log("resolve budget exhausted before attempt elapsed_ms=" + swPipe.ElapsedMilliseconds);
                return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null);
            }
            using var ctsResolve = new CancellationTokenSource(TimeSpan.FromMilliseconds(remainingForAttempt));

            req.WrapperDeadlineMs = (int)Math.Max(0, totalDeadlineMs - swPipe.ElapsedMilliseconds - 1000);

            byte[] payload;
            try { payload = SerializeWithTrailingNewline(req); }
            catch (Exception ex) { Log("request serialize failed: " + ex.Message); return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null); }

            var swSend = Stopwatch.StartNew();
            try
            {
                // Single WriteAsync containing payload + '\n'. Earlier impl
                // split this across two WriteAsync calls + an explicit
                // FlushAsync -- three kernel transitions per pipe send. Named
                // pipes don't buffer like FileStream so the explicit Flush
                // was already redundant; the byte-mode pipe with PIPE_WAIT
                // dispatches the write atomically.
                await pipe.WriteAsync(payload, ctsResolve.Token).ConfigureAwait(false);
                // Subtract 1 from the logged byte count so the existing log
                // ("request sent ... bytes=...") keeps reporting the JSON
                // payload length and stays comparable across the change.
                Log("request sent id=" + req.Id[..8] + " bytes=" + (payload.Length - 1) + " player=" + player
                    + " wrapper_deadline_ms=" + req.WrapperDeadlineMs
                    + " elapsed_ms=" + swSend.ElapsedMilliseconds);
            }
            catch (Exception ex) { Log("pipe write failed: " + ex.GetType().Name + ": " + ex.Message); return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null); }

            var swRead = Stopwatch.StartNew();
            string? line;
            try { line = await ReadLineAsync(pipe, ctsResolve.Token).ConfigureAwait(false); }
            catch (OperationCanceledException) { Log("pipe read TIMED OUT after " + swRead.ElapsedMilliseconds + " ms (no terminal frame within " + (int)ResolveDeadline.TotalSeconds + " s)"); return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null); }
            catch (Exception ex) { Log("pipe read failed: " + ex.GetType().Name + ": " + ex.Message); return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null); }
            if (string.IsNullOrEmpty(line)) { Log("pipe returned empty response after " + swRead.ElapsedMilliseconds + " ms"); return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null); }

            Log("response received bytes=" + line.Length + " elapsed_ms=" + swRead.ElapsedMilliseconds);

            ResolveResponse? resp;
            try { resp = JsonSerializer.Deserialize(line, WrapperJsonContext.Default.ResolveResponse); }
            catch (Exception ex) { Log("response parse failed: " + ex.GetType().Name + ": " + ex.Message); return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null); }
            if (resp == null) { Log("response was null after deserialize"); return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null); }

            if (resp.Action == WireConstants.ActionResolved && !string.IsNullOrEmpty(resp.Url))
            {
                // AVPro sanity check (defense-in-depth). The server's
                // FormatSelectorBuilder filters codecs upstream; if a domain
                // config regression slips through and hands us an .flv / rtmp /
                // .f4v URL while the player is AVPro, AVPro will silently fail
                // playback. Exec og instead -- yt-dlp can often pick a working
                // format itself when the server's strategy was wrong.
                if (player == WireConstants.PlayerAvPro && !IsAvProCompatibleUrl(resp.Url))
                {
                    Log("response action=resolved id=" + (resp.Id ?? "?")[..Math.Min(8, (resp.Id ?? "?").Length)] +
                        " but URL fails AVPro shape check (host=" + ExtractHost(resp.Url) + ") -- falling back");
                    return (null, WireConstants.OgFallbackReasonAvProIncompatible, null);
                }
                Log("response action=resolved id=" + (resp.Id ?? "?")[..Math.Min(8, (resp.Id ?? "?").Length)] + " url-host=" + ExtractHost(resp.Url));
                return (resp.Url, null, null);
            }

            if (resp.Action == WireConstants.ActionFallbackNative)
            {
                long elapsedSinceRequestMs = swRequest.ElapsedMilliseconds;
                long remainingBudgetMs = totalDeadlineMs - swPipe.ElapsedMilliseconds;
                Log("response action=fallback_native id=" + (resp.Id ?? "?")[..Math.Min(8, (resp.Id ?? "?").Length)]
                    + " reason=" + (resp.Reason ?? "?")
                    + " elapsed_ms_since_request_sent=" + elapsedSinceRequestMs
                    + " remaining_budget_ms=" + remainingBudgetMs);

                if (ResolveRetryPolicy.ShouldRetry(resp.Reason, retriesSent, remainingBudgetMs))
                {
                    int delayMs = ResolveRetryPolicy.NextDelayMs(retriesSent);
                    retriesSent++;
                    Log("wrapper_retry_attempt attempt=" + retriesSent
                        + " delay_ms=" + delayMs
                        + " elapsed_ms_since_request_sent=" + elapsedSinceRequestMs
                        + " remaining_budget_ms=" + remainingBudgetMs);
                    try { await Task.Delay(delayMs, ctsResolve.Token).ConfigureAwait(false); }
                    catch (OperationCanceledException)
                    {
                        Log("retry delay cancelled (deadline elapsed)");
                        return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null);
                    }
                    // Reuse same request id for server dedup/correlation.
                    continue;
                }

                // Capture the server's public_message (when present) so the watchdog can
                // surface a yellow line explaining why we punted -- DRM-detected,
                // sign-in required, etc.
                return (null, WireConstants.OgFallbackReasonServerFallbackNative, resp.PublicMessage);
            }

            Log("response action UNKNOWN: " + resp.Action);
            return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null);
        }
    }

    // Serialize the request DTO with a trailing '\n' framing byte appended
    // in-place. Source-gen path (WrapperJsonContext) writes through a
    // PooledByteBufferWriter under the hood; we copy out and append the
    // newline so the wire send is one WriteAsync instead of two.
    private static byte[] SerializeWithTrailingNewline(ResolveRequest req)
    {
        byte[] body = JsonSerializer.SerializeToUtf8Bytes(req, WrapperJsonContext.Default.ResolveRequest);
        byte[] framed = new byte[body.Length + 1];
        Buffer.BlockCopy(body, 0, framed, 0, body.Length);
        framed[body.Length] = (byte)'\n';
        return framed;
    }

    // v3.2: notify the watchdog that we're falling back to og.exe so it
    // can surface a single console line in real time. Fire-and-forget --
    // open a fresh pipe, write one JSON-NDJSON line, close. Any failure
    // (watchdog not running, pipe busy, etc.) is silently swallowed: og
    // fallback must never fail because diagnostic IPC failed. Tight
    // timeout (1.5 s) so a hung watchdog can't delay the og exec.
    private static async Task TrySendOgFallbackNotifyAsync(string? url, string reason, long elapsedMs, string? publicMessage = null)
    {
        try
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(1500));
            using var pipe = new NamedPipeClientStream(
                ".",
                WireConstants.PipeName,
                PipeDirection.InOut,
                PipeOptions.Asynchronous);
            try { await pipe.ConnectAsync(cts.Token).ConfigureAwait(false); }
            catch { return; /* watchdog not running -- og still runs, no diagnostic line */ }

            var notify = new WrapperEventNotify
            {
                Action = WireConstants.ActionOgFallbackNotify,
                Url = url,
                Reason = reason,
                ElapsedMs = elapsedMs,
                Rid = s_rid,
                PublicMessage = publicMessage,
            };
            byte[] body = JsonSerializer.SerializeToUtf8Bytes(notify, WrapperJsonContext.Default.WrapperEventNotify);
            byte[] framed = new byte[body.Length + 1];
            Buffer.BlockCopy(body, 0, framed, 0, body.Length);
            framed[body.Length] = (byte)'\n';
            try { await pipe.WriteAsync(framed, cts.Token).ConfigureAwait(false); }
            catch { /* fire-and-forget */ }
        }
        catch { /* fire-and-forget */ }
    }

    // Mirror of TrySendOgFallbackNotifyAsync for the og-failed signal.
    // Carries exit code + stderr preview so the watchdog can decide
    // whether to evict the cache entry and surface a console line.
    // Fire-and-forget; bounded 1.5s budget.
    private static async Task TrySendOgFailedNotifyAsync(string? url, string reason, int exitCode, string errorPreview, long elapsedMs)
    {
        try
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(1500));
            using var pipe = new NamedPipeClientStream(
                ".",
                WireConstants.PipeName,
                PipeDirection.InOut,
                PipeOptions.Asynchronous);
            try { await pipe.ConnectAsync(cts.Token).ConfigureAwait(false); }
            catch { return; }

            var notify = new WrapperEventNotify
            {
                Action = WireConstants.ActionWrapperOgFailedNotify,
                Url = url,
                Reason = reason,
                ExitCode = exitCode,
                ErrorPreview = errorPreview,
                ElapsedMs = elapsedMs,
                Rid = s_rid,
            };
            byte[] body = JsonSerializer.SerializeToUtf8Bytes(notify, WrapperJsonContext.Default.WrapperEventNotify);
            byte[] framed = new byte[body.Length + 1];
            Buffer.BlockCopy(body, 0, framed, 0, body.Length);
            framed[body.Length] = (byte)'\n';
            try { await pipe.WriteAsync(framed, cts.Token).ConfigureAwait(false); }
            catch { /* fire-and-forget */ }
        }
        catch { /* fire-and-forget */ }
    }

    // Map og's stderr to a coarse failure-reason tag. The patterns cover
    // the common upstream blockers we've seen in the field: Cloudflare
    // anti-bot (403), rate limits (429), YouTube/auth-gated sign-in prompts,
    // and "this content does not exist" terminal errors (deleted videos,
    // closed channels, private videos, 404/410). Anything else is "unknown"
    // -- still worth a notify so the user sees something failed.
    //
    // content_not_found matches the server-side reason emitted by
    // DomainDispatcher.IsContentNotFoundFailure; aligning the two lets a
    // watchdog skip a redundant og spawn when the server already classified
    // the URL as terminally absent.
    private static string ClassifyOgFailure(string stderr)
    {
        if (string.IsNullOrEmpty(stderr)) return "unknown";
        if (stderr.Contains("HTTP Error 403", StringComparison.OrdinalIgnoreCase)) return "cf_403";
        if (stderr.Contains("HTTP Error 429", StringComparison.OrdinalIgnoreCase)) return "rate_limited";
        if (stderr.Contains("Sign in to confirm", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("Sign in to verify", StringComparison.OrdinalIgnoreCase)) return "sign_in_required";
        if (stderr.Contains("Video unavailable", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("This video is no longer available", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("This video is not available", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("removed by the uploader", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("closed their YouTube account", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("account associated", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("Private video", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("HTTP Error 404", StringComparison.OrdinalIgnoreCase)
            || stderr.Contains("HTTP Error 410", StringComparison.OrdinalIgnoreCase)) return "content_not_found";
        return "unknown";
    }

    // Buffered NDJSON read. Mirrors LocalIpcServer's read loop so a single
    // response line is consumed up to the first '\n'.
    private static async Task<string?> ReadLineAsync(Stream s, CancellationToken ct)
    {
        using var ms = new MemoryStream();
        var buf = new byte[4096];
        bool sawNewline = false;
        const int MaxResponseBytes = 4 * 1024 * 1024;
        while (ms.Length < MaxResponseBytes)
        {
            int n = await s.ReadAsync(buf.AsMemory(), ct).ConfigureAwait(false);
            if (n == 0) break;
            int consume = n;
            int nlIdx = Array.IndexOf(buf, (byte)'\n', 0, n);
            if (nlIdx >= 0) { sawNewline = true; consume = nlIdx; }
            for (int i = 0; i < consume && ms.Length < MaxResponseBytes; i++)
            {
                byte b = buf[i];
                if (b == (byte)'\r') continue;
                ms.WriteByte(b);
            }
            if (sawNewline) break;
        }
        if (ms.Length == 0) return null;
        return Encoding.UTF8.GetString(ms.GetBuffer(), 0, (int)ms.Length);
    }

    // Exec sibling yt-dlp-og.exe with the same argv. yt-dlp-og.exe lives
    // next to us — VRChat's Tools dir is our cwd when invoked, and
    // PatchManager preserves vanilla yt-dlp there as yt-dlp-og.exe before
    // installing this wrapper. We capture og's stdout + stderr (so the log
    // sees what it actually emitted), then forward both to OUR own
    // stdout/stderr unmodified before exiting with og's exit code. VRChat's
    // pipe sees identical bytes to what it would see if vanilla yt-dlp had
    // been invoked directly — that's the graceful-degradation contract.
    //
    // When yt-dlp-og.exe is missing entirely, we emit EMPTY stdout + exit 0.
    // VRChat's resolver sees "no URL" and falls into its own error path,
    // matching what would happen if Tools dir were broken / vrcresolver
    // uninstalled mid-runtime. NEVER emit the input URL as if we'd resolved
    // it — that confuses VRChat into "resolved to same URL" and the player
    // tries to open a watch-page URL it can't decode.
    private static async Task<int> ExecFallbackAsync(string[] args, string? url)
    {
        string exeDir = AppContext.BaseDirectory;

        // Recursive-exec guard. Any candidate that classifies as one of our own
        // wrappers (current build, a prior release in the hash list, or a dev
        // build with the embedded marker) would loop back into this same code
        // path, so it is skipped. Hash list lives in LocalLow (the watchdog
        // stages it on startup); marker + PE-info signals cover dev builds even
        // when the list is missing.
        string knownHashesPath = Path.Combine(AppPaths.StateRoot(), "wrapper_hashes.txt");

        // Pick a native yt-dlp to hand off to. FallbackBinary.Select walks, in order,
        // yt-dlp-og.exe next to us, yt-dlp-og.exe in VRChat's Tools dir, then VRChat's
        // own yt-dlp.exe there -- each gated so we never recurse into one of our own
        // wrappers. Hash list lives in LocalLow (the watchdog stages it on startup);
        // marker + PE-info signals cover dev builds even when the list is missing.
        // Without this, a missing og made the wrapper emit empty stdout, which the user
        // sees as a blank video for the whole resolve budget on any mesh outage.
        string? vrcTools = VrcPathLocator.Find();
        string? ogPath = FallbackBinary.Select(
            exeDir,
            vrcTools,
            File.Exists,
            p => WrapperIdentity.Classify(p, knownHashesPath) == WrapperKind.Ours);

        if (ogPath == null)
        {
            Log("FALLBACK no-og: no usable vanilla yt-dlp found -- emitting empty stdout, exit 0");
            return 0;
        }

        Log("FALLBACK og: spawning " + ogPath + " with " + args.Length + " args");
        var sw = Stopwatch.StartNew();
        var psi = new ProcessStartInfo
        {
            FileName = ogPath,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = false,
            CreateNoWindow = true,
            WorkingDirectory = Path.GetDirectoryName(ogPath) ?? exeDir,
        };
        foreach (var a in args) psi.ArgumentList.Add(a);

        try
        {
            using var proc = Process.Start(psi);
            if (proc == null) { Log("FALLBACK og: Process.Start returned null"); return 0; }

            // Read both streams concurrently to avoid pipe-buffer deadlock
            // on yt-dlp invocations that emit non-trivial stderr (warnings,
            // bot-detect notices) while stdout is still streaming.
            var stdoutTask = proc.StandardOutput.ReadToEndAsync();
            var stderrTask = proc.StandardError.ReadToEndAsync();
            // Bounded wait. og is VRChat's own yt-dlp and normally exits in
            // seconds; a wedged og would otherwise pin this wrapper (and
            // VRChat's resolve) forever, one zombie pair per video.
            using var ctsOg = new CancellationTokenSource(TimeSpan.FromMinutes(5));
            try
            {
                await proc.WaitForExitAsync(ctsOg.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                Log("FALLBACK og: no exit after 5 min -- killing process tree, emitting empty stdout");
                try { proc.Kill(entireProcessTree: true); } catch { /* already gone */ }
                try { await proc.WaitForExitAsync().ConfigureAwait(false); } catch { /* reaped */ }
                return 0;
            }
            string ogStdout = await stdoutTask.ConfigureAwait(false);
            string ogStderr = await stderrTask.ConfigureAwait(false);
            sw.Stop();

            Log("FALLBACK og: exit=" + proc.ExitCode + " elapsed_ms=" + sw.ElapsedMilliseconds + " stdout_bytes=" + ogStdout.Length + " stderr_bytes=" + ogStderr.Length);
            if (ogStdout.Length > 0)
                Log("FALLBACK og stdout-preview: " + Preview(ogStdout, 240));
            if (ogStderr.Length > 0)
                Log("FALLBACK og stderr-preview: " + Preview(ogStderr, 240));

            // og itself failed. Notify the watchdog so it can evict any
            // stale cache entry for this URL and surface "og also failed"
            // on the console. Fire-and-forget; pure diagnostic channel.
            if (proc.ExitCode != 0 && !string.IsNullOrEmpty(url))
            {
                string failureReason = ClassifyOgFailure(ogStderr);
                string preview = ogStderr.Length > 0 ? Preview(ogStderr.Trim(), 200) : "";
                await TrySendOgFailedNotifyAsync(url, failureReason, proc.ExitCode, preview, sw.ElapsedMilliseconds).ConfigureAwait(false);
            }

            // Pass through to our stdout/stderr so VRChat sees exactly what
            // vanilla yt-dlp would emit. Bytes copied verbatim — no encoding
            // mangling, no trailing-newline normalization, no BOM.
            if (ogStdout.Length > 0)
            {
                using var ourStdout = Console.OpenStandardOutput();
                byte[] bytes = Encoding.UTF8.GetBytes(ogStdout);
                ourStdout.Write(bytes, 0, bytes.Length);
                ourStdout.Flush();
            }
            if (ogStderr.Length > 0)
            {
                using var ourStderr = Console.OpenStandardError();
                byte[] bytes = Encoding.UTF8.GetBytes(ogStderr);
                ourStderr.Write(bytes, 0, bytes.Length);
                ourStderr.Flush();
            }

            return proc.ExitCode;
        }
        catch (Exception ex)
        {
            sw.Stop();
            Log("FALLBACK og: exec failed: " + ex.GetType().Name + ": " + ex.Message + " elapsed_ms=" + sw.ElapsedMilliseconds);
            // Graceful degradation — if og itself can't be spawned, give
            // VRChat empty stdout + exit 0 instead of bubbling the exception.
            return 0;
        }
    }

    // VRChat's stdout reader is line-buffered + picky: exactly one '\n'
    // terminator, no CRLF, no BOM. Don't use Console.WriteLine.
    // Swallows write failures (VRChat closed its end of the pipe) -- there is
    // nobody left to read the URL and an unhandled IOException here used to
    // crash the wrapper with a stack trace on stderr.
    private static void TryWriteUrlToStdout(string url)
    {
        try
        {
            string output = url.Trim() + "\n";
            byte[] bytes = Encoding.UTF8.GetBytes(output);
            using var stdout = Console.OpenStandardOutput();
            stdout.Write(bytes, 0, bytes.Length);
            stdout.Flush();
        }
        catch (Exception ex)
        {
            Log("stdout write failed: " + ex.GetType().Name + ": " + ex.Message);
        }
    }

    // First arg starting with http:// or https:// is the URL to resolve.
    // Quoted matches are stripped of surrounding quotes by the OS argv
    // parse before we see them.
    private static string ExtractUrl(string[] args)
    {
        foreach (var a in args)
        {
            if (a.StartsWith("http://", StringComparison.OrdinalIgnoreCase) ||
                a.StartsWith("https://", StringComparison.OrdinalIgnoreCase))
                return a;
        }
        return "";
    }

    // Returns the value following "-f" or "--format", or null if absent.
    // Matches the form `-f <selector>` and `--format <selector>`; does NOT
    // attempt to handle `--format=<selector>` (VRChat uses the spaced form).
    private static string? ExtractDashFValue(string[] args)
    {
        for (int i = 0; i < args.Length - 1; i++)
        {
            if (args[i] == "-f" || args[i] == "--format")
                return args[i + 1];
        }
        return null;
    }

    // Bare host or "?" if the input isn't a parseable absolute URL. Used
    // for log lines where the full URL would carry user-identifying tokens
    // (YouTube video ids, twitch streams, etc.).
    private static string ExtractHost(string url)
    {
        if (string.IsNullOrEmpty(url)) return "?";
        try
        {
            if (Uri.TryCreate(url, UriKind.Absolute, out var u)) return u.Host;
        }
        catch { /* best-effort */ }
        return "?";
    }

    // Blacklist check: reject the formats AVPro can't decode (rtmp/rtmps
    // schemes, .flv/.f4v containers). Trust by default -- this is a
    // backstop against a server-side regression, not an allowlist of
    // every legitimate CDN URL shape.
    private static bool IsAvProCompatibleUrl(string url)
    {
        if (string.IsNullOrEmpty(url)) return false;
        string lower = url.ToLowerInvariant();
        if (lower.StartsWith("rtmp://", StringComparison.Ordinal)
            || lower.StartsWith("rtmps://", StringComparison.Ordinal)) return false;
        int q = lower.IndexOf('?');
        string pathLower = q >= 0 ? lower.Substring(0, q) : lower;
        if (pathLower.EndsWith(".flv", StringComparison.Ordinal)
            || pathLower.EndsWith(".f4v", StringComparison.Ordinal)) return false;
        return true;
    }
}
