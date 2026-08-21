using System.Net.WebSockets;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class MeshClient
{
    private async Task RunLoopAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            try
            {
                string node = await ResolveNodeHostAsync(ct).ConfigureAwait(false);
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

                var wsUri = ServerEndpoints.MeshWebSocketUrlForHost(node);
                await _ws.ConnectAsync(wsUri, ct).ConfigureAwait(false);

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

                _cachedNodeHost = node;
                _useApexDiscoveryFallback = false;
                _wasConnected = true;
                _firstReconnectFailureUtc = DateTime.MinValue;
                _lastPongUtc = DateTime.UtcNow;

                ArmWelcomeTimeout(_welcomeTcs!, ct);

                ConsoleUx.Write(LogComponent.Mesh, "connected node=" + node);

                await PumpAsync(ct).ConfigureAwait(false);
                ConsoleUx.Write(LogComponent.Mesh, "disconnected (clean close)");
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested) { break; }
            catch (Exception ex)
            {
                if (_wasConnected || _reconnectAttempt == 0)
                {
                    ConsoleUx.Warn(LogComponent.Mesh, "disconnected (error): "
                        + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 160));
                }
                _wasConnected = false;
                FailAllPending(WireConstants.FallbackServerUnreachable);
                if (string.Equals(_currentNodeHost, ServerEndpoints.ProxyHost, StringComparison.OrdinalIgnoreCase)
                    && string.IsNullOrEmpty(_cachedNodeHost))
                {
                    _useApexDiscoveryFallback = true;
                }
                if (_firstReconnectFailureUtc == DateTime.MinValue)
                    _firstReconnectFailureUtc = DateTime.UtcNow;
                if (DateTime.UtcNow - _firstReconnectFailureUtc > ApexReResolveAfter)
                {
                    _cachedNodeHost = null;
                    _useApexDiscoveryFallback = false;
                }
            }
            finally
            {
                _welcomeTcs?.TrySetResult(null);
                // Clean close (server sent a Close frame) exits PumpAsync
                // without throwing, so the catch above never runs -- without
                // this, in-flight resolves sat parked until the IPC budget
                // fired. Idempotent with the catch path's call.
                FailAllPending(WireConstants.FallbackServerUnreachable);
                try { _ws?.Dispose(); } catch { /* ignore */ }
                _ws = null;
            }

            if (ct.IsCancellationRequested) break;

            _reconnectAttempt++;
            WatchdogStats.RecordReconnect();
            int capSec = ReconnectCapsSec[Math.Min(_reconnectAttempt - 1, ReconnectCapsSec.Length - 1)];
            int waitMs = _rng.Next(0, capSec * 1000 + 1);
            if (_reconnectAttempt <= 5 || _reconnectAttempt % 10 == 0)
                ConsoleUx.Write(LogComponent.Mesh, $"reconnect attempt {_reconnectAttempt} in {waitMs / 1000} s");

            try { await Task.Delay(waitMs, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { break; }
        }
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

    private async Task<string> ResolveNodeHostAsync(CancellationToken ct)
    {
        if (!string.IsNullOrEmpty(_cachedNodeHost)) return _cachedNodeHost!;
        if (!_useApexDiscoveryFallback) return ServerEndpoints.ProxyHost;

        Exception? lastEx = null;
        for (int attempt = 1; attempt <= 3; attempt++)
        {
            try
            {
                using var req = new HttpRequestMessage(HttpMethod.Get, ServerEndpoints.ApexDiscoveryUrl);
                req.Headers.UserAgent.ParseAdd(_userAgent);
                using var resp = await _httpClient.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, ct).ConfigureAwait(false);
                if ((int)resp.StatusCode >= 300 && (int)resp.StatusCode < 400 && resp.Headers.Location != null)
                {
                    if (ServerEndpoints.TryExtractDiscoveryRedirectHost(resp.Headers.Location, out string host))
                    {
                        return host;
                    }
                }
                lastEx = new InvalidOperationException("apex returned " + (int)resp.StatusCode + " with no usable Location");
            }
            catch (Exception ex) { lastEx = ex; }
            try { await Task.Delay(1000 * attempt, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { throw; }
        }

        FailAllPending(WireConstants.FallbackServerUnreachable);
        ConsoleUx.Warn(LogComponent.Mesh, "apex discovery failed -- falling back to native yt-dlp until reconnect succeeds.");
        throw lastEx ?? new InvalidOperationException("apex discovery failed");
    }

    private async Task PumpAsync(CancellationToken ct)
    {
        var pumpCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
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
                            try { _ws?.Abort(); } catch { /* ignore */ }
                            return;
                        }
                        break;
                }
            }
        }
        finally
        {
            pumpCts.Cancel();
            try { await hbTask.ConfigureAwait(false); } catch { /* ignore */ }
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
            catch { return; }
            try { await Task.Delay(PongDeadline, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }

            if (_lastPongUtc < sentAt)
            {
                try { ws.Abort(); } catch { /* ignore */ }
                return;
            }
        }
    }
}
