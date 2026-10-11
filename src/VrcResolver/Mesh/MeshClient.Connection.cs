using System.Diagnostics;
using System.Net.WebSockets;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class MeshClient
{
    private async Task RunLoopAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            Volatile.Write(ref _lastAttemptStartTicks, DateTime.UtcNow.Ticks);
            MeshLink? link = null;
            HttpMessageInvoker? invoker = null;
            var probe = _connectProbe = new MeshConnectProbe();
            var connectWatch = new Stopwatch();
            bool socketOpened = false;
            try
            {
                string node = ServerEndpoints.ProxyHost;
                PrepareWelcomeState(node);

                _ws = new ClientWebSocket();
                _ws.Options.SetRequestHeader("User-Agent", _userAgent);
                _ws.Options.KeepAliveInterval = TimeSpan.Zero;
                _ws.Options.AddSubProtocol(WireConstants.SubprotocolV3);
                _ws.Options.DangerousDeflateOptions = new WebSocketDeflateOptions
                {
                    ClientMaxWindowBits = 15,
                    ServerMaxWindowBits = 15,
                };

                var wsUri = _meshUriForHost(node);
                invoker = new HttpMessageInvoker(MeshConnectProbe.CreateHandler(probe), disposeHandler: true);
                _connectInFlight = true;
                connectWatch.Start();
                using (var connectCts = CancellationTokenSource.CreateLinkedTokenSource(ct))
                {
                    connectCts.CancelAfter(_connectDeadline);
                    await _ws.ConnectAsync(wsUri, invoker, connectCts.Token).ConfigureAwait(false);
                }
                connectWatch.Stop();
                socketOpened = true;
                Logger.WriteFileOnly("[mesh] connected in " + connectWatch.ElapsedMilliseconds + " ms"
                    + FormatTimings(probe));

                _isV3Connection = ShouldSendClientHello(_ws.SubProtocol);
                Logger.WriteFileOnly("[mesh][v3] negotiated subprotocol="
                    + (_ws.SubProtocol ?? "<none>")
                    + " v3=" + _isV3Connection
                    + " deflate-offered=true");

                if (_isV3Connection)
                {
                    try { await SendClientHelloAsync(node, ct).ConfigureAwait(false); }
                    catch (Exception ex)
                    {
                        Logger.WriteFileOnly("[mesh][v3] client_hello send failed: "
                            + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 160));
                    }
                }

                _wasConnected = true;
                _lastPongUtc = DateTime.UtcNow;

                ArmWelcomeTimeout(_welcomeTcs!, ct);
                var readyLink = link = new MeshLink();
                var linkWs = _ws;
                _ = _welcomeTcs!.Task.ContinueWith(_ => MarkLinkReady(readyLink, linkWs), TaskScheduler.Default);

                ConsoleUx.Write(LogComponent.Mesh, "connected node=" + node);

                await PumpAsync(ct).ConfigureAwait(false);
                ConsoleUx.Write(LogComponent.Mesh, "disconnected (clean close)");
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested) { break; }
            catch (Exception ex)
            {
                string failure = socketOpened
                    ? "disconnected (error): " + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 160)
                    : DescribeConnectFailure(_currentNodeHost, connectWatch.ElapsedMilliseconds, probe, ex);
                if (_wasConnected || _reconnectAttempt == 0)
                    ConsoleUx.Warn(LogComponent.Mesh, failure);
                else
                    Logger.WriteFileOnly("[mesh] reconnect attempt " + _reconnectAttempt + " failed: " + failure);
                _wasConnected = false;
            }
            finally
            {
                MarkLinkDown(link);
                SettleConnectAttempt();
                _welcomeTcs?.TrySetResult(null);
                try { _ws?.Dispose(); } catch { }
                _ws = null;
                invoker?.Dispose();
            }

            if (ct.IsCancellationRequested) break;

            _reconnectAttempt++;
            WatchdogStats.RecordReconnect();
            int waitMs = _nextBackoffMs(_reconnectAttempt);
            if (_reconnectAttempt <= 5 || _reconnectAttempt % 10 == 0)
                ConsoleUx.Write(LogComponent.Mesh, $"reconnect attempt {_reconnectAttempt} in {waitMs / 1000} s");

            try { await WaitBeforeReconnectAsync(waitMs, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { break; }
        }
    }

    private async Task WaitBeforeReconnectAsync(int waitMs, CancellationToken ct)
    {
        var wake = Volatile.Read(ref _wake).Task;
        var backoffEnd = DateTime.UtcNow.AddMilliseconds(waitMs);
        using var delayCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        var delay = Task.Delay(waitMs, delayCts.Token);
        var first = await Task.WhenAny(delay, wake).ConfigureAwait(false);
        delayCts.Cancel();
        if (first == delay)
        {
            await delay.ConfigureAwait(false);
        }
        else
        {
            var wakeAt = new DateTime(Volatile.Read(ref _lastAttemptStartTicks), DateTimeKind.Utc) + MinWakeSpacing;
            if (wakeAt > backoffEnd) wakeAt = backoffEnd;
            var pause = wakeAt - DateTime.UtcNow;
            if (pause > TimeSpan.Zero)
                await Task.Delay(pause, ct).ConfigureAwait(false);
        }
        ct.ThrowIfCancellationRequested();
        Interlocked.Exchange(ref _wake, NewSignal());
    }

    private static TaskCompletionSource NewSignal() => new(TaskCreationOptions.RunContinuationsAsynchronously);

    private void WakeReconnectLoop() => Volatile.Read(ref _wake).TrySetResult();

    private void SettleConnectAttempt()
    {
        _connectInFlight = false;
        Interlocked.Exchange(ref _connectSettled, NewSignal()).TrySetResult();
    }

    internal static string DescribeConnectFailure(string host, long elapsedMs, MeshConnectProbe probe, Exception ex)
    {
        string where = " after " + elapsedMs + " ms during " + probe.Phase + FormatTimings(probe);
        if (ex is OperationCanceledException)
            return "connect to " + host + " timed out" + where;

        Exception root = ex.GetBaseException();
        string cause = ReferenceEquals(root, ex)
            ? ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 160)
            : ex.GetType().Name + " (" + root.GetType().Name + ": " + LogUtil.SanitizeForConsole(root.Message, 160) + ")";
        return "connect to " + host + " failed" + where + ": " + cause;
    }

    private static string FormatTimings(MeshConnectProbe probe)
    {
        var parts = new List<string>(2);
        if (probe.DnsMs >= 0) parts.Add("dns " + probe.DnsMs + " ms");
        if (probe.TcpMs >= 0) parts.Add("tcp " + probe.TcpMs + " ms");
        return parts.Count == 0 ? "" : " (" + string.Join(", ", parts) + ")";
    }

    private void PrepareWelcomeState(string node)
    {
        _serverProtocolVersion = 0;
        _serverNode = null;
        _serverFeatures = null;
        _warpActive = null;
        _serverVersion = null;
        _ytDlpVersion = null;
        _isV3Connection = false;
        _negotiatedFormat = WireConstants.FormatJson;
        _isMsgpackFormat = false;
        _currentNodeHost = node;
        _welcomeTcs = new TaskCompletionSource<WelcomeFrame?>(TaskCreationOptions.RunContinuationsAsynchronously);
    }

    private void MarkLinkReady(MeshLink link, ClientWebSocket? ws)
    {
        if (link.Down.Task.IsCompleted || ws is not { State: WebSocketState.Open }) return;
        link.WasReady = true;
        Interlocked.Exchange(ref _wake, NewSignal());
        Volatile.Write(ref _readyLink, link);
        Interlocked.Exchange(ref _linkReady,
            new TaskCompletionSource<MeshLink>(TaskCreationOptions.RunContinuationsAsynchronously)).TrySetResult(link);
        SettleConnectAttempt();
    }

    private void MarkLinkDown(MeshLink? link)
    {
        if (link == null) return;
        Interlocked.CompareExchange(ref _readyLink, null, link);
        if (link.WasReady)
            Interlocked.Exchange(ref _linkDownSinceTicks, DateTime.UtcNow.Ticks);
        link.Down.TrySetResult();
    }

    private async Task<MeshLink?> WaitForLinkAsync(Task pending, CancellationToken ct)
    {
        bool woke = false;
        while (true)
        {
            var next = Volatile.Read(ref _linkReady).Task;
            var link = Volatile.Read(ref _readyLink);
            if (link != null && !link.Down.Task.IsCompleted) return link;

            if (!woke)
            {
                WakeReconnectLoop();
                woke = true;
            }

            var settled = Volatile.Read(ref _connectSettled).Task;
            var remaining = new DateTime(Interlocked.Read(ref _linkDownSinceTicks), DateTimeKind.Utc)
                + _reconnectGrace - DateTime.UtcNow;
            bool graceRanOut = remaining <= TimeSpan.Zero;
            if (graceRanOut && !_connectInFlight) return null;

            using var delayCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            var delay = Task.Delay(graceRanOut ? Timeout.InfiniteTimeSpan : remaining, delayCts.Token);
            var done = graceRanOut
                ? await Task.WhenAny(next, pending, delay, settled).ConfigureAwait(false)
                : await Task.WhenAny(next, pending, delay).ConfigureAwait(false);
            delayCts.Cancel();
            if (done == pending) return null;
            if (done == delay)
            {
                ct.ThrowIfCancellationRequested();
                if (graceRanOut) return null;
                continue;
            }
            if (done == settled)
            {
                var ready = Volatile.Read(ref _readyLink);
                return ready != null && !ready.Down.Task.IsCompleted ? ready : null;
            }
            woke = false;
        }
    }

    private void ArmWelcomeTimeout(TaskCompletionSource<WelcomeFrame?> welcomeTcs, CancellationToken ct)
    {
        _ = Task.Delay(WelcomeTimeout, ct).ContinueWith(_ =>
        {
            if (_welcomeTcs != welcomeTcs) return;
            if (welcomeTcs.TrySetResult(null))
            {
                Interlocked.CompareExchange(ref _serverProtocolVersion, 1, 0);
                Logger.WriteFileOnly("[mesh] no welcome within 1s -- assuming v1 server");
            }
        }, TaskScheduler.Default);
    }

    private async Task PumpAsync(CancellationToken ct)
    {
        using var pumpCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        Task hbTask = HeartbeatLoopAsync(pumpCts.Token);

        try
        {
            var buf = new Memory<byte>(new byte[64 * 1024]);
            using var ms = new MemoryStream();
            while (!pumpCts.IsCancellationRequested)
            {
                ms.SetLength(0);
                ValueWebSocketReceiveResult r;
                do
                {
                    r = await _ws!.ReceiveAsync(buf, pumpCts.Token).ConfigureAwait(false);
                    if (r.MessageType == WebSocketMessageType.Close) return;
                    ms.Write(buf.Span[..r.Count]);
                    if (ms.Length > 4 * 1024 * 1024) throw new InvalidOperationException("frame too large");
                } while (!r.EndOfMessage);

                switch (r.MessageType)
                {
                    case WebSocketMessageType.Text:
                        await DispatchFrameAsync(ms.ToArray(), pumpCts.Token).ConfigureAwait(false);
                        break;
                    case WebSocketMessageType.Binary:
                        if (_isMsgpackFormat)
                        {
                            await DispatchBinaryFrameAsync(ms.ToArray(), pumpCts.Token).ConfigureAwait(false);
                        }
                        else
                        {
                            ConsoleUx.Warn(LogComponent.Mesh, "unexpected Binary frame on json-negotiated connection -- aborting + reconnecting");
                            try { _ws?.Abort(); } catch { }
                            return;
                        }
                        break;
                }
            }
        }
        finally
        {
            pumpCts.Cancel();
            try { await hbTask.ConfigureAwait(false); } catch { }
        }
    }

    private async Task HeartbeatLoopAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            try { await Task.Delay(HeartbeatInterval, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }

            var ws = _ws;
            if (ws is not { State: WebSocketState.Open }) return;
            DateTime sentAt = DateTime.UtcNow;
            try { await SendTextFrameAsync(PingFrame, ct).ConfigureAwait(false); }
            catch
            {
                try { ws.Abort(); } catch { }
                return;
            }
            try { await Task.Delay(PongDeadline, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }

            if (_lastPongUtc < sentAt)
            {
                try { ws.Abort(); } catch { }
                return;
            }
        }
    }
}
