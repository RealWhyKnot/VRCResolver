using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

[SupportedOSPlatform("windows")]
internal static partial class Program
{
    private static readonly TimeSpan PipeConnectTimeout = TimeSpan.FromSeconds(1);
    private static readonly TimeSpan ResolveDeadline = TimeSpan.FromSeconds(28);

    private static string s_rid = "????????";
    private static int? s_serverRetryAfterMs;

    private static async Task<int> Main(string[] args)
    {
        s_rid = Guid.NewGuid().ToString("N")[..8];
        var swTotal = Stopwatch.StartNew();

        try
        {
            string url = ExtractUrl(args);
            string? formatArg = ResolveRequestProfile.ExtractDashFValue(args);
            string player = ResolveRequestProfile.InferPlayer(formatArg);

            LogStartBanner(args, url, formatArg, player);

            int exitCode;
            string outcome;

            if (string.IsNullOrEmpty(url))
            {
                Log("no URL in argv -- exec og fallback (diagnostic invocation)");
                await TrySendOgFallbackNotifyAsync(null, WireConstants.OgFallbackReasonNoUrlDiagnostic, swTotal.ElapsedMilliseconds).ConfigureAwait(false);
                exitCode = await ExecFallbackAsync(args, null).ConfigureAwait(false);
                outcome = "no-url-fallback";
            }
            else
            {
                if (FirstPartyUrlPolicy.IsFirstPartyPlaybackProxyUrl(url))
                {
                    string toEmit = TryWrapForTrustGateway(url, probeRelay: true);
                    TryWriteUrlToStdout(toEmit);
                    bool wrapped = !string.Equals(toEmit, url, StringComparison.Ordinal);
                    Log("direct first-party playback URL host=" + LogUtil.BareHost(url)
                        + " emitted-host=" + LogUtil.BareHost(toEmit)
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

                    if (!string.IsNullOrEmpty(result.resolved)
                        && !ResolvedUrlGuard.IsSafeToEmit(result.resolved))
                    {
                        Log("resolved URL fails emit guard (host=" + LogUtil.BareHost(result.resolved!) + ") -- falling back");
                        result = (null, WireConstants.OgFallbackReasonResolvedUrlRejected, null);
                    }

                    if (!string.IsNullOrEmpty(result.resolved))
                    {
                        string toEmit = TryWrapForTrustGateway(result.resolved!);
                        TryWriteUrlToStdout(toEmit);
                        bool wrapped = !ReferenceEquals(toEmit, result.resolved);
                        Log("emitted resolved URL to stdout host=" + LogUtil.BareHost(toEmit)
                            + " bytes=" + toEmit.Length
                            + " trust_gateway=" + (wrapped ? "wrapped" : "passthrough"));
                        exitCode = 0;
                        outcome = wrapped ? "pipe-resolved-wrapped" : "pipe-resolved";
                    }
                    else
                    {
                        string reason = result.fallbackReason ?? WireConstants.OgFallbackReasonPipeResolveFailed;
                        await TrySendOgFallbackNotifyAsync(url, reason, swTotal.ElapsedMilliseconds, result.publicMessage).ConfigureAwait(false);

                        exitCode = await ExecFallbackAsync(args, url,
                            ogFailureReason => ReAskServerAsync(url, player, formatArg, swTotal, ogFailureReason)).ConfigureAwait(false);
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

    private static async Task<(string? Url, string? FallbackReason, string? PublicMessage)> ResolveOverPipeAsync(string url, string player, string? formatArg, TimeSpan? deadlineOverride = null, bool skipNativeHint = false)
    {
        var swPipe = Stopwatch.StartNew();
        long totalDeadlineMs = (long)(deadlineOverride ?? ResolveDeadline).TotalMilliseconds;
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
            SkipNativeHint = skipNativeHint ? true : null,
        };

        var swRequest = Stopwatch.StartNew();
        int connectRetriesSent = 0;
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
                if (connectRetriesSent == 0)
                    Log("pipe connect OK elapsed_ms=" + swPipe.ElapsedMilliseconds);
                else
                    Log("pipe reconnect OK retry=" + connectRetriesSent + " elapsed_ms=" + swPipe.ElapsedMilliseconds);
            }
            catch (Exception ex)
            {
                Log(ex switch
                {
                    OperationCanceledException => "pipe connect TIMED OUT after " + swPipe.ElapsedMilliseconds + " ms (watchdog not running?)",
                    System.IO.FileNotFoundException => "pipe connect ENOENT (watchdog not running)",
                    _ => "pipe connect failed: " + ex.GetType().Name + ": " + ex.Message,
                });
                if (connectRetriesSent == 0 && totalDeadlineMs - swPipe.ElapsedMilliseconds > 2000)
                {
                    connectRetriesSent++;
                    Log("pipe connect retry in 750 ms");
                    await Task.Delay(750).ConfigureAwait(false);
                    continue;
                }
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
                await pipe.WriteAsync(payload, ctsResolve.Token).ConfigureAwait(false);
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
                if (player == WireConstants.PlayerAvPro && !ResolvedUrlGuard.IsAvProCompatibleUrl(resp.Url))
                {
                    Log("response action=resolved id=" + (resp.Id ?? "?")[..Math.Min(8, (resp.Id ?? "?").Length)] +
                        " but URL fails AVPro shape check (host=" + LogUtil.BareHost(resp.Url) + ") -- falling back");
                    return (null, WireConstants.OgFallbackReasonAvProIncompatible, null);
                }
                Log("response action=resolved id=" + (resp.Id ?? "?")[..Math.Min(8, (resp.Id ?? "?").Length)] + " url-host=" + LogUtil.BareHost(resp.Url));
                return (resp.Url, null, null);
            }

            if (resp.Action == WireConstants.ActionFallbackNative)
            {
                s_serverRetryAfterMs = resp.RetryAfterMs;
                Log("response action=fallback_native id=" + (resp.Id ?? "?")[..Math.Min(8, (resp.Id ?? "?").Length)]
                    + " reason=" + (resp.Reason ?? "?")
                    + " retry_after_ms=" + (resp.RetryAfterMs?.ToString() ?? "-")
                    + " elapsed_ms_since_request_sent=" + swRequest.ElapsedMilliseconds
                    + " remaining_budget_ms=" + (totalDeadlineMs - swPipe.ElapsedMilliseconds));

                return (null, WireConstants.OgFallbackReasonServerFallbackNative, resp.PublicMessage);
            }

            Log("response action UNKNOWN: " + resp.Action);
            return (null, WireConstants.OgFallbackReasonPipeResolveFailed, null);
        }
    }

    private static byte[] SerializeWithTrailingNewline(ResolveRequest req)
    {
        byte[] body = JsonSerializer.SerializeToUtf8Bytes(req, WrapperJsonContext.Default.ResolveRequest);
        byte[] framed = new byte[body.Length + 1];
        Buffer.BlockCopy(body, 0, framed, 0, body.Length);
        framed[body.Length] = (byte)'\n';
        return framed;
    }

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
            catch { return; }

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
            catch { }
        }
        catch { }
    }

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
            catch { }
        }
        catch { }
    }

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

    private static async Task<string?> ReAskServerAsync(
        string url, string player, string? formatArg, Stopwatch swTotal, string ogFailureReason)
    {
        if (ogFailureReason == "content_not_found")
        {
            Log("re-ask skipped: content gone upstream");
            return null;
        }
        long remainingMs = (long)ResolveDeadline.TotalMilliseconds - swTotal.ElapsedMilliseconds;
        if (remainingMs < 3000)
        {
            Log("re-ask skipped: remaining_budget_ms=" + remainingMs);
            return null;
        }
        if (s_serverRetryAfterMs is int hint)
        {
            if (hint > remainingMs - 2000)
            {
                Log("re-ask skipped: server retry_after_ms=" + hint + " remaining_budget_ms=" + remainingMs);
                return null;
            }
            if (hint > 0)
            {
                Log("re-ask waiting delay_ms=" + hint + " per server hint");
                await Task.Delay(hint).ConfigureAwait(false);
                remainingMs = (long)ResolveDeadline.TotalMilliseconds - swTotal.ElapsedMilliseconds;
            }
        }
        Log("re-ask start remaining_budget_ms=" + remainingMs);
        var retry = await ResolveOverPipeAsync(url, player, formatArg,
            TimeSpan.FromMilliseconds(Math.Max(2000, remainingMs - 250)),
            skipNativeHint: true).ConfigureAwait(false);
        if (string.IsNullOrEmpty(retry.Url))
        {
            Log("re-ask did not resolve reason=" + (retry.FallbackReason ?? "?"));
            return null;
        }
        if (!ResolvedUrlGuard.IsSafeToEmit(retry.Url))
        {
            Log("re-ask resolved URL fails emit guard host=" + LogUtil.BareHost(retry.Url!));
            return null;
        }
        return TryWrapForTrustGateway(retry.Url!);
    }

    private static async Task<int> ExecFallbackAsync(string[] args, string? url, Func<string, Task<string?>>? reAskAsync = null)
    {
        string exeDir = AppContext.BaseDirectory;

        string knownHashesPath = Path.Combine(AppPaths.StateRoot(), "wrapper_hashes.txt");

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

            var stdoutTask = proc.StandardOutput.ReadToEndAsync();
            var stderrTask = proc.StandardError.ReadToEndAsync();
            using var ctsOg = new CancellationTokenSource(TimeSpan.FromMinutes(5));
            try
            {
                await proc.WaitForExitAsync(ctsOg.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                Log("FALLBACK og: no exit after 5 min -- killing process tree, emitting empty stdout");
                try { proc.Kill(entireProcessTree: true); } catch { }
                try { await proc.WaitForExitAsync().ConfigureAwait(false); } catch { }
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

            if (proc.ExitCode != 0 && !string.IsNullOrEmpty(url))
            {
                string failureReason = ClassifyOgFailure(ogStderr);
                string preview = ogStderr.Length > 0 ? Preview(ogStderr.Trim(), 200) : "";
                await TrySendOgFailedNotifyAsync(url, failureReason, proc.ExitCode, preview, sw.ElapsedMilliseconds).ConfigureAwait(false);

                if (reAskAsync != null)
                {
                    string? retryUrl = null;
                    try { retryUrl = await reAskAsync(failureReason).ConfigureAwait(false); }
                    catch (Exception rex) { Log("re-ask threw: " + rex.GetType().Name + ": " + rex.Message); }
                    if (!string.IsNullOrEmpty(retryUrl))
                    {
                        Log("re-ask resolved after og failure host=" + LogUtil.BareHost(retryUrl!)
                            + " suppressed_og_stderr_bytes=" + ogStderr.Length);
                        TryWriteUrlToStdout(retryUrl!);
                        return 0;
                    }
                }
            }

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
            return 0;
        }
    }

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

}
