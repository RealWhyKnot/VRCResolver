using System.Diagnostics;
using System.Net;
using System.Runtime.Versioning;
using System.Text;
using VrcResolver.Shared;

namespace VrcResolver;

// Local-relay listener at http(s)://127.0.0.1:{port}/. Trust gateway
// for AVPro: VRChat's AVPro player ships a built-in trusted-host
// allowlist that includes `*.youtube.com`. The hosts file maps
// `localhost.youtube.com -> 127.0.0.1`, so AVPro fetches
//   http(s)://localhost.youtube.com:{port}/play/<session>/manifest.<ext>?target=<base64>
// AVPro's trust check passes on the localhost.youtube.com host; the listener
// decodes target=, forwards to the real upstream (typically
// `https://vrcresolver.com/api/proxy?...`), and streams bytes back.
//
// If the server-side playlist uses relative subresource URLs, AVPro resolves
// them under /play/<session>/ and the listener forwards them relative to the
// original target URL's directory. When the server emits absolute first-party
// playback proxy URLs, the relay localizes only those URLs back under the
// same localhost namespace. The server still owns manifest compatibility,
// tier routing, and transcoding.
//
// The relay never rewrites arbitrary upstream URLs. Only first-party
// proxy URLs are accepted or localized, so localhost cannot become a general
// open proxy for local processes.
//
// HTTPS is preferred when the per-machine certificate + Windows HTTP.sys
// binding are available. HTTP remains a fallback so declining UAC does not
// break playback on worlds that still allow plain local loopback URLs.
//
// AOT-clean: HttpListener + HttpClient + System.IO. No reflection, no dynamic codegen.
[SupportedOSPlatform("windows")]
internal sealed partial class LocalRelayServer : IDisposable
{
    private static readonly TimeSpan UpstreamHeaderDeadline = TimeSpan.FromSeconds(30);
    private static readonly TimeSpan BodyIdleTimeout = TimeSpan.FromSeconds(30);
    private const int CopyBufferSize = 80 * 1024;

    private readonly int _port;
    private readonly string _scheme;
    private readonly HttpListener _listener;
    private readonly CancellationTokenSource _cts = new();
    private readonly HttpClient _http;
    private readonly LocalRelayTargetResolver _targets = new();
    private Task? _acceptLoop;

    // Verbose request/response trace gated on BuildInfo.IsDevBuild. Dev builds
    // (auto-version OR dirty source) emit per-request logs the user can paste
    // back when reporting an issue; release builds (clean -Version + clean
    // tree, what CI ships) stay quiet so the watchdog log isn't flooded
    // during normal use.
    private static readonly bool s_verbose = BuildInfo.IsDevBuild;
    private static long s_reqCounter;
    // Bounds the fire-and-forget handler tasks (and their upstream
    // connections). Real HLS fan-out is a handful of segment requests;
    // triple digits means something local is hammering the listener.
    private const int MaxInFlight = 128;
    private int _inFlight;

    public LocalRelayServer(int port, string scheme = "http")
    {
        _port = port;
        _scheme = TrustGatewayUrlBuilder.IsAllowedGatewayScheme(scheme)
            ? scheme.ToLowerInvariant()
            : "http";
        _listener = new HttpListener();
        _listener.Prefixes.Add(_scheme + "://127.0.0.1:" + port + "/");
        var handler = new SocketsHttpHandler
        {
            AllowAutoRedirect = true,
            MaxAutomaticRedirections = 5,
            PooledConnectionIdleTimeout = TimeSpan.FromSeconds(60),
            ConnectTimeout = TimeSpan.FromSeconds(15),
            // Address guard at connect time -- covers every redirect hop,
            // which the pre-flight target allowlist cannot.
            ConnectCallback = GuardedRelayConnect.Callback,
        };
        _http = new HttpClient(handler) { Timeout = Timeout.InfiniteTimeSpan };
    }

    public void Start()
    {
        try
        {
            _listener.Start();
        }
        catch (HttpListenerException ex)
        {
            ConsoleUx.Error(LogComponent.Relay, "listener failed on port "
                + _port + " scheme=" + _scheme + ": " + ex.Message);
            throw;
        }
        _acceptLoop = Task.Run(AcceptLoopAsync);
    }

    public async Task StopAsync()
    {
        _cts.Cancel();
        try { _listener.Stop(); } catch { /* best-effort */ }
        if (_acceptLoop != null)
        {
            try { await _acceptLoop.ConfigureAwait(false); } catch { /* ignore */ }
        }
    }

    private async Task AcceptLoopAsync()
    {
        while (_listener.IsListening && !_cts.IsCancellationRequested)
        {
            HttpListenerContext ctx;
            try { ctx = await _listener.GetContextAsync().ConfigureAwait(false); }
            catch (HttpListenerException) { break; }
            catch (ObjectDisposedException) { break; }
            catch (Exception ex)
            {
                if (_cts.IsCancellationRequested) break;
                ConsoleUx.Warn(LogComponent.Relay, "accept failed: " + ex.Message);
                continue;
            }
            // Fire-and-forget per-request handler. HttpListener serialises
            // accept; the dispatched task does the actual work in parallel.
            if (Interlocked.Increment(ref _inFlight) > MaxInFlight)
            {
                Interlocked.Decrement(ref _inFlight);
                try { ctx.Response.StatusCode = 503; ctx.Response.Close(); } catch { /* best-effort */ }
                continue;
            }
            _ = Task.Run(async () =>
            {
                try { await HandleAsync(ctx).ConfigureAwait(false); }
                finally { Interlocked.Decrement(ref _inFlight); }
            });
        }
    }

    private async Task HandleAsync(HttpListenerContext ctx)
    {
        using var reqCts = CancellationTokenSource.CreateLinkedTokenSource(_cts.Token);
        reqCts.CancelAfter(UpstreamHeaderDeadline);
        string targetUrl = "";
        string path = "";
        string method = "?";
        int upstreamStatusCode = 0;
        string? lazyHlsState = null;
        long lazyHlsWaitMs = -1;
        string? lazyHlsGenerator = null;
        string? failure = null;
        var sw = Stopwatch.StartNew();
        string reqId = s_verbose
            ? Interlocked.Increment(ref s_reqCounter).ToString("x6", System.Globalization.CultureInfo.InvariantCulture)
            : "";
        long t0 = s_verbose ? Environment.TickCount64 : 0;
        long upstreamHeaderMs = -1;
        long bytesOut = 0;
        try
        {
            path = ctx.Request.Url?.AbsolutePath ?? "";
            method = ctx.Request.HttpMethod ?? "?";
            if (s_verbose)
            {
                string rawUrl = ctx.Request.RawUrl ?? "";
                string remote = ctx.Request.RemoteEndPoint?.ToString() ?? "?";
                string protoVer = "HTTP/" + (ctx.Request.ProtocolVersion?.ToString() ?? "?");
                Verbose("req=" + reqId + " <- " + method + " " + ShortUrl(rawUrl)
                    + " from=" + remote + " " + protoVer);
                DumpHeaders("[relay] req=" + reqId + "  H<", ctx.Request.Headers);
            }
            if (!string.Equals(method, "GET", StringComparison.OrdinalIgnoreCase)
                && !string.Equals(method, "HEAD", StringComparison.OrdinalIgnoreCase))
            {
                if (s_verbose) Verbose("req=" + reqId + " -> 405 (method)");
                ctx.Response.StatusCode = 405;
                ctx.Response.Headers["Allow"] = "GET, HEAD";
                ctx.Response.Close();
                return;
            }

            // Media players never send Origin; a browser on this machine
            // does. The hosts-file pin makes localhost.youtube.com reachable
            // from any local page, so drive-by fetch attempts are rejected
            // outright rather than relayed.
            if (ctx.Request.Headers["Origin"] != null)
            {
                if (s_verbose) Verbose("req=" + reqId + " -> 403 (origin header)");
                ctx.Response.StatusCode = 403;
                ctx.Response.Close();
                return;
            }

            if (!LocalRelaySecurity.IsAllowedHostHeader(ctx.Request.Headers["Host"], _port))
            {
                if (s_verbose) Verbose("req=" + reqId + " -> 403 (host)");
                ctx.Response.StatusCode = 403;
                ctx.Response.Close();
                return;
            }

            if (!LocalRelayTargetResolver.IsPlayPath(path))
            {
                if (s_verbose) Verbose("req=" + reqId + " -> 404 (path mismatch)");
                ctx.Response.StatusCode = 404;
                ctx.Response.Close();
                return;
            }

            if (!_targets.TryResolve(
                    path,
                    ctx.Request.Url?.Query ?? "",
                    ctx.Request.QueryString["target"],
                    out LocalRelayTarget target))
            {
                if (s_verbose) Verbose("req=" + reqId + " -> 400 (no target param or relative base)");
                ctx.Response.StatusCode = 400;
                ctx.Response.Close();
                return;
            }

            targetUrl = target.Url;
            if (!LocalRelaySecurity.IsAllowedTargetUrl(targetUrl, out string targetRejectReason))
            {
                if (s_verbose) Verbose("req=" + reqId + " -> 403 ("
                    + targetRejectReason + " target='" + ShortUrl(targetUrl) + "')");
                ctx.Response.StatusCode = 403;
                ctx.Response.Close();
                return;
            }
            if (s_verbose) Verbose("req=" + reqId + " kind=" + target.Kind + " target=" + ShortUrl(targetUrl));
            WatchdogStats.RecordRelayRequest(targetUrl);

            using var outboundReq = new HttpRequestMessage(
                new HttpMethod(method), targetUrl);
            LocalRelayHeaders.ForwardRequestHeaders(ctx.Request, outboundReq);

            using var resp = await _http.SendAsync(
                outboundReq,
                HttpCompletionOption.ResponseHeadersRead,
                reqCts.Token).ConfigureAwait(false);
            upstreamHeaderMs = sw.ElapsedMilliseconds;
            upstreamStatusCode = (int)resp.StatusCode;
            lazyHlsState = FirstHeader(resp, "X-Lazy-HLS");
            lazyHlsWaitMs = ParseLongHeader(resp, "X-Lazy-HLS-Wait-Ms");
            lazyHlsGenerator = FirstHeader(resp, "X-Lazy-HLS-Generator");

            if (s_verbose)
            {
                Verbose("req=" + reqId + " upstream-status=" + (int)resp.StatusCode
                    + " ms=" + (Environment.TickCount64 - t0));
                DumpHttpHeaders("[relay] req=" + reqId + "  upstream-H>", resp.Headers, resp.Content.Headers);
            }

            bool isHead = string.Equals(method, "HEAD", StringComparison.OrdinalIgnoreCase);
            bool shouldLocalizeManifest = !isHead
                && resp.IsSuccessStatusCode
                && LocalRelayManifestLocalizer.IsLikelyManifest(
                    path,
                    targetUrl,
                    resp.Content.Headers.ContentType);

            if (shouldLocalizeManifest)
            {
                long? upstreamContentLength = resp.Content.Headers.ContentLength;
                if (s_verbose
                    && upstreamContentLength.HasValue
                    && upstreamContentLength.Value > LocalRelayManifestLocalizer.MaxManifestBytes)
                {
                    Verbose("req=" + reqId + " upstream manifest unusually large len="
                        + upstreamContentLength.Value);
                }

                using var manifestStream = await resp.Content.ReadAsStreamAsync(_cts.Token).ConfigureAwait(false);

                ctx.Response.StatusCode = (int)resp.StatusCode;
                CopyResponseHeaders(
                    ctx.Response,
                    resp,
                    contentLengthOverride: null,
                    contentTypeOverride: resp.Content.Headers.ContentType?.ToString()
                        ?? "application/vnd.apple.mpegurl",
                    bodyRewritten: true,
                    out int passedManifestHeaders,
                    out int droppedManifestHeaders);
                ctx.Response.SendChunked = true;

                Encoding? inputEncoding = TryResolveCharsetEncoding(
                    resp.Content.Headers.ContentType?.CharSet);
                var localizeResult = await LocalRelayManifestLocalizer.LocalizeStreamAsync(
                    manifestStream,
                    inputEncoding,
                    ctx.Response.OutputStream,
                    LocalRelayManifestLocalizer.MaxManifestBytes,
                    _cts.Token).ConfigureAwait(false);
                if (localizeResult.Exceeded)
                {
                    ConsoleUx.Warn(LogComponent.Relay,
                        "manifest body exceeded sanity cap; truncated upstream="
                        + ShortUrl(targetUrl));
                }
                if (s_verbose)
                {
                    Verbose("req=" + reqId + " localized-manifest chars-in="
                        + localizeResult.CharsIn + " chars-out=" + localizeResult.CharsOut
                        + " changed=" + localizeResult.Changed
                        + (localizeResult.Exceeded ? " EXCEEDED" : ""));
                    Verbose("req=" + reqId + " response-headers passed="
                        + passedManifestHeaders + " dropped=" + droppedManifestHeaders
                        + " ct='" + ctx.Response.ContentType + "' chunked=true");
                    DumpHeaders("[relay] req=" + reqId + "  H>", ctx.Response.Headers);
                }
                bytesOut = localizeResult.CharsOut;
                WatchdogStats.RecordRelayBytes(targetUrl, bytesOut);
                return;
            }

            ctx.Response.StatusCode = (int)resp.StatusCode;
            CopyResponseHeaders(
                ctx.Response,
                resp,
                contentLengthOverride: resp.Content.Headers.ContentLength,
                contentTypeOverride: resp.Content.Headers.ContentType?.ToString(),
                bodyRewritten: false,
                out int passedHeaders,
                out int droppedHeaders);
            if (s_verbose) Verbose("req=" + reqId + " response-headers passed=" + passedHeaders
                + " dropped=" + droppedHeaders + " ct='" + ctx.Response.ContentType + "' len="
                + (resp.Content.Headers.ContentLength?.ToString() ?? "null"));

            if (s_verbose) DumpHeaders("[relay] req=" + reqId + "  H>", ctx.Response.Headers);

            if (isHead)
            {
                if (s_verbose) Verbose("req=" + reqId + " -> " + ctx.Response.StatusCode
                    + " HEAD elapsed=" + (Environment.TickCount64 - t0) + "ms");
                return;
            }

            // Binary stream copy with per-read idle CTS so a long video
            // (HLS segment, mp4 progressive) doesn't get killed by a
            // single deadline timer; idle = 30s without a byte = abort.
            using var upstream = await resp.Content.ReadAsStreamAsync(_cts.Token).ConfigureAwait(false);
            byte[] buf = new byte[CopyBufferSize];
            while (!_cts.IsCancellationRequested)
            {
                using var idle = CancellationTokenSource.CreateLinkedTokenSource(_cts.Token);
                idle.CancelAfter(BodyIdleTimeout);
                int n;
                try { n = await upstream.ReadAsync(buf.AsMemory(), idle.Token).ConfigureAwait(false); }
                catch (OperationCanceledException) when (!_cts.IsCancellationRequested && idle.IsCancellationRequested)
                {
                    failure = "body_idle_timeout";
                    ConsoleUx.Warn(LogComponent.Relay, "stream idle timeout for " + ShortUrl(targetUrl));
                    return;
                }
                if (n == 0) break;
                await ctx.Response.OutputStream.WriteAsync(buf.AsMemory(0, n), _cts.Token).ConfigureAwait(false);
                bytesOut += n;
                WatchdogStats.RecordRelayBytes(targetUrl, n);
            }
            if (s_verbose) Verbose("req=" + reqId + " -> " + ctx.Response.StatusCode
                + " stream bytes-out=" + bytesOut + " elapsed=" + (Environment.TickCount64 - t0) + "ms");
        }
        catch (HttpListenerException hle)
        {
            failure = "client_disconnect";
            if (s_verbose) Verbose("req=" + reqId + " client-disconnect (HttpListenerException ec=" + hle.ErrorCode + ") bytes-out=" + bytesOut);
        }
        catch (System.IO.IOException ioe)
        {
            failure = "client_disconnect";
            if (s_verbose) Verbose("req=" + reqId + " client-disconnect (IOException: " + ioe.Message + ") bytes-out=" + bytesOut);
        }
        catch (OperationCanceledException) when (_cts.IsCancellationRequested) { /* shutdown */ }
        catch (Exception ex)
        {
            failure = ex.GetType().Name;
            ConsoleUx.Warn(LogComponent.Relay, "request failed for " + ShortUrl(targetUrl)
                + ": " + ex.GetType().Name + ": " + ex.Message);
            try { ctx.Response.StatusCode = 502; } catch { }
        }
        finally
        {
            if (!string.IsNullOrWhiteSpace(targetUrl))
            {
                LocalRelayHitchDetector.Record(new LocalRelayTimingSample(
                    method,
                    path,
                    targetUrl,
                    upstreamStatusCode,
                    upstreamHeaderMs,
                    sw.ElapsedMilliseconds,
                    bytesOut,
                    lazyHlsState,
                    lazyHlsWaitMs,
                    lazyHlsGenerator,
                    failure));
            }

            try { ctx.Response.Close(); } catch { /* best-effort */ }
        }
    }

    private static void CopyResponseHeaders(
        HttpListenerResponse downstream,
        HttpResponseMessage upstream,
        long? contentLengthOverride,
        string? contentTypeOverride,
        bool bodyRewritten,
        out int passedHeaders,
        out int droppedHeaders)
    {
        passedHeaders = 0;
        droppedHeaders = 0;
        foreach (var h in upstream.Headers.Concat(upstream.Content.Headers))
        {
            if (LocalRelayHeaders.ShouldDropResponseHeader(h.Key)) { droppedHeaders++; continue; }
            if (string.Equals(h.Key, "Content-Length", StringComparison.OrdinalIgnoreCase)) continue;
            if (string.Equals(h.Key, "Content-Type", StringComparison.OrdinalIgnoreCase)) continue;
            if (bodyRewritten && string.Equals(h.Key, "Content-Encoding", StringComparison.OrdinalIgnoreCase))
            {
                droppedHeaders++;
                continue;
            }

            try { downstream.Headers.Add(h.Key, string.Join(", ", h.Value)); passedHeaders++; }
            catch { /* HttpListener rejects some restricted headers; skip */ }
        }

        if (!string.IsNullOrWhiteSpace(contentTypeOverride))
            downstream.ContentType = contentTypeOverride;
        if (contentLengthOverride.HasValue)
            downstream.ContentLength64 = contentLengthOverride.Value;
    }

    private static Encoding? TryResolveCharsetEncoding(string? charset)
    {
        if (string.IsNullOrWhiteSpace(charset)) return null;
        try { return Encoding.GetEncoding(charset.Trim('"')); }
        catch { return null; }
    }

    private static void DumpHeaders(string prefix, System.Collections.Specialized.NameValueCollection headers)
    {
        if (headers == null) return;
        foreach (string? key in headers.AllKeys)
        {
            if (string.IsNullOrEmpty(key)) continue;
            string val = headers[key] ?? "";
            if (val.Length > 200) val = val.Substring(0, 200) + "...";
            Logger.WriteFileOnly(prefix + " " + key + ": " + val);
        }
    }

    private static void DumpHttpHeaders(string prefix, System.Net.Http.Headers.HttpResponseHeaders respH, System.Net.Http.Headers.HttpContentHeaders contentH)
    {
        foreach (var h in respH.Concat(contentH))
        {
            string val = string.Join(", ", h.Value);
            if (val.Length > 200) val = val.Substring(0, 200) + "...";
            Logger.WriteFileOnly(prefix + " " + h.Key + ": " + val);
        }
    }

    private static string? FirstHeader(HttpResponseMessage response, string name)
    {
        if (response.Headers.TryGetValues(name, out var values))
            return values.FirstOrDefault();
        if (response.Content.Headers.TryGetValues(name, out values))
            return values.FirstOrDefault();
        return null;
    }

    private static long ParseLongHeader(HttpResponseMessage response, string name)
    {
        string? value = FirstHeader(response, name);
        return long.TryParse(value, System.Globalization.NumberStyles.Integer, System.Globalization.CultureInfo.InvariantCulture, out long parsed)
            ? parsed
            : -1;
    }

    private static void Verbose(string message)
    {
        Logger.WriteDiagnostic(LogComponent.Relay, "[relay] " + message, message);
    }

    private static string ShortUrl(string url)
    {
        if (string.IsNullOrEmpty(url)) return "?";
        return url.Length > 100 ? url.Substring(0, 100) + "..." : url;
    }

    public void Dispose()
    {
        _cts.Cancel();
        try { _listener.Close(); } catch { /* best-effort */ }
        _http.Dispose();
        _cts.Dispose();
    }
}
