using System.Net.WebSockets;
using System.Text.Json;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class MeshClient
{
    private async Task DispatchFrameAsync(byte[] payload, CancellationToken ct)
    {
        JsonDocument doc;
        try { doc = JsonDocument.Parse(payload); }
        catch (Exception ex)
        {
            // Server protocol regression / framing bug. Without this log a
            // malformed frame would drop pending TCS to time out 10s later as
            // server_unreachable with no breadcrumb. Dedupe by exception type
            // so a flapping server can't fill the scrollback.
            LogParseFailure(ex, payload);
            return;
        }

        string action = "";
        if (doc.RootElement.TryGetProperty("action", out var actionEl) && actionEl.ValueKind == JsonValueKind.String)
            action = actionEl.GetString() ?? "";

        switch (action)
        {
            case WireConstants.ActionResolved:
            case WireConstants.ActionFallbackNative:
                {
                    // Extract id, reason, and (on `resolved`) the resolved URL
                    // from the parsed doc; then dispose the doc and hand the
                    // verified raw frame bytes to the pending TCS. Caller writes
                    // bytes through to the pipe — no JsonDocument re-encode on
                    // the hot path.
                    string id = "";
                    if (doc.RootElement.TryGetProperty("id", out var idEl) && idEl.ValueKind == JsonValueKind.String)
                        id = idEl.GetString() ?? "";

                    string? reason = null;
                    if (doc.RootElement.TryGetProperty("reason", out var reasonEl) && reasonEl.ValueKind == JsonValueKind.String)
                        reason = reasonEl.GetString();

                    string? resolvedUrl = null;
                    if (action == WireConstants.ActionResolved
                        && doc.RootElement.TryGetProperty("url", out var urlEl)
                        && urlEl.ValueKind == JsonValueKind.String)
                    {
                        resolvedUrl = urlEl.GetString();
                    }

                    // bytes_estimate is a v2 response field (server's stream-size
                    // estimate). Sum across resolves for the heartbeat line so
                    // the operator can see aggregate "stream-bytes" served. Only
                    // counted on `resolved` (fallback_native means og takes over
                    // and the bytes don't go through us).
                    if (action == WireConstants.ActionResolved
                        && doc.RootElement.TryGetProperty("bytes_estimate", out var beEl)
                        && beEl.ValueKind == JsonValueKind.Number
                        && beEl.TryGetInt64(out long bytesEstimate))
                    {
                        WatchdogStats.RecordBytesEstimate(bytesEstimate);
                    }

                    if (action == WireConstants.ActionFallbackNative)
                        LogFallbackNative(id, reason);

                    // doc no longer needed past this point — payload bytes carry
                    // everything LocalIpcServer needs to forward to the wrapper.
                    doc.Dispose();

                    if (string.IsNullOrEmpty(id)) return;

                    // Pop the inflight cid so VrcLogMonitor can later look it up by
                    // the resolved URL. Only on `resolved` — fallback_native means
                    // the patched yt-dlp re-runs vanilla, so the URL AVPro
                    // ultimately opens isn't ours to attribute.
                    _inflightCids.TryRemove(id, out var cid);
                    if (action == WireConstants.ActionResolved
                        && !string.IsNullOrEmpty(cid)
                        && !string.IsNullOrEmpty(resolvedUrl))
                    {
                        RememberResolvedUrlCid(resolvedUrl!, cid);
                    }

                    if (_pending.TryRemove(id, out var tcs))
                    {
                        if (action == WireConstants.ActionResolved) _reconnectAttempt = 0;
                        tcs.TrySetResult(new MeshResolveResult(payload, action, reason));
                    }
                    return;
                }
            case WireConstants.ActionWelcome:
                {
                    WelcomeFrame? welcome = null;
                    try { welcome = JsonSerializer.Deserialize(payload, MeshJsonContext.Default.WelcomeFrame); }
                    catch (Exception ex)
                    {
                        ConsoleUx.Warn(LogComponent.Mesh, "welcome parse failed -- assuming v1 server: "
                            + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 160));
                        // Pin protocol version to v1 so subsequent ResolveAsync calls
                        // don't get stuck waiting for a welcome that never arrives in
                        // a parseable form.
                        Interlocked.CompareExchange(ref _serverProtocolVersion, 1, 0);
                    }

                    if (welcome != null)
                    {
                        // Clamp to [1, ClientProtocolVersion]:
                        //   - 0 / missing field demoting us back to "pre-welcome"
                        //     would re-arm the 1s-timer's CompareExchange branch
                        //     and confuse routing decisions. Force at least 1.
                        //   - We can't speak anything newer than ClientProtocolVersion;
                        //     advertising support for v999 would be a lie.
                        int negotiated = Math.Clamp(
                            welcome.ProtocolVersion,
                            1,
                            WireConstants.ClientProtocolVersion);
                        Interlocked.Exchange(ref _serverProtocolVersion, negotiated);
                        _serverNode = welcome.Node;
                        _serverFeatures = welcome.Features;
                        _warpActive = welcome.WarpActive;
                        _serverVersion = welcome.ServerVersion;
                        _ytDlpVersion = welcome.YtDlpVersion;

                        // v3.1: capture the post-welcome wire format the
                        // server picked from our accept_formats list. Null
                        // / missing field = "json" (v3.0 server, or v3.1
                        // server we sent json-only opt-out to).
                        _negotiatedFormat = welcome.NegotiatedFormat ?? WireConstants.FormatJson;
                        _isMsgpackFormat = string.Equals(_negotiatedFormat, WireConstants.FormatMsgpack, StringComparison.Ordinal);
                        Logger.WriteFileOnly("[mesh][v3.1] negotiated_format=" + _negotiatedFormat
                            + " isMsgpack=" + _isMsgpackFormat);

                        if (welcome.Features == null)
                            ConsoleUx.Warn(LogComponent.Mesh, "welcome missing required field: features");

                        // Feature welcome_hosts: adopt the server's host/path lists so the
                        // relay policy stops depending on this build's hardcoded set. The
                        // policy validates and UNIONS -- a hostile or broken list can only
                        // add candidate hosts that still have to pass every other guard.
                        if (HasFeature(welcome.Features, WireConstants.FeatureWelcomeHosts))
                            FirstPartyUrlPolicy.SetServerProvided(welcome.FirstPartyHosts, welcome.PlaybackProxyPaths);

                        string features = welcome.Features != null && welcome.Features.Length > 0
                            ? "[" + string.Join(",", welcome.Features) + "]"
                            : "[]";
                        ConsoleUx.Write(
                            LogComponent.Mesh,
                            "welcome node=" + (welcome.Node ?? "?") +
                            " v=" + welcome.ProtocolVersion + " (negotiated=" + negotiated + ")" +
                            " server=" + (welcome.ServerVersion ?? "?") +
                            " yt-dlp=" + (welcome.YtDlpVersion ?? "?") +
                            " warp_active=" + (welcome.WarpActive?.ToString() ?? "?") +
                            " features=" + LogUtil.SanitizeForConsole(features, 240));

                        // v3: persist the welcome contents keyed by hash so
                        // the next reconnect can offer it back in client_hello
                        // and let the server reply with the smaller
                        // welcome_cached frame. Only on v3 connections AND
                        // when the server actually sent a hash — v2 servers
                        // don't and shouldn't cache.
                        if (_isV3Connection && !string.IsNullOrEmpty(welcome.WelcomeHash)
                            && !string.IsNullOrEmpty(_currentNodeHost))
                        {
                            try { _welcomeCache.Store(_currentNodeHost, welcome, welcome.WelcomeHash!); }
                            catch (Exception storeEx)
                            {
                                Logger.WriteFileOnly("[mesh][v3] cache store failed: "
                                    + storeEx.GetType().Name + ": "
                                    + LogUtil.SanitizeForConsole(storeEx.Message, 160));
                            }
                        }
                    }
                    _welcomeTcs?.TrySetResult(welcome);
                    doc.Dispose();
                    return;
                }
            case WireConstants.ActionWelcomeCached:
                {
                    // v3: server confirmed our cached welcome_hash matched.
                    // Hydrate per-connection state from the local cache;
                    // server only sent the dynamic fields (warp_active +
                    // node label) in this small frame. Engines / features /
                    // version strings come from the cache entry.
                    if (!_isV3Connection)
                    {
                        ConsoleUx.Warn(LogComponent.Mesh, "welcome_cached received on non-v3 connection -- protocol error, reconnecting");
                        try { _ws?.Abort(); } catch { /* ignore */ }
                        doc.Dispose();
                        return;
                    }
                    WelcomeCachedFrame? cached = null;
                    try { cached = JsonSerializer.Deserialize(payload, MeshJsonContext.Default.WelcomeCachedFrame); }
                    catch (Exception ex)
                    {
                        ConsoleUx.Warn(LogComponent.Mesh, "welcome_cached parse failed: "
                            + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 160));
                        Interlocked.CompareExchange(ref _serverProtocolVersion, 1, 0);
                        _welcomeTcs?.TrySetResult(null);
                        doc.Dispose();
                        return;
                    }

                    var entry = !string.IsNullOrEmpty(_currentNodeHost)
                        ? _welcomeCache.Get(_currentNodeHost)
                        : null;
                    if (entry == null)
                    {
                        // Server claimed cache hit but we have nothing to
                        // hydrate from — sync drift (file deleted between
                        // client_hello send and this dispatch, or another
                        // process clobbered the cache). Drop any stale
                        // slot and force a clean reconnect with null hash;
                        // the server will resend the full welcome.
                        if (!string.IsNullOrEmpty(_currentNodeHost))
                            _welcomeCache.Invalidate(_currentNodeHost);
                        ConsoleUx.Warn(LogComponent.Mesh, "welcome_cached but local entry missing -- invalidating + reconnecting");
                        try { _ws?.Abort(); } catch { /* ignore */ }
                        doc.Dispose();
                        return;
                    }

                    int negotiated = Math.Clamp(
                        cached?.ProtocolVersion ?? entry.ProtocolVersion,
                        1,
                        WireConstants.ClientProtocolVersion);
                    Interlocked.Exchange(ref _serverProtocolVersion, negotiated);
                    _serverNode = cached?.Node ?? entry.Node;
                    _serverFeatures = entry.Features;
                    _warpActive = cached?.WarpActive ?? entry.WarpActive;
                    _serverVersion = entry.ServerVersion;
                    _ytDlpVersion = entry.YtDlpVersion;
                    if (HasFeature(entry.Features, WireConstants.FeatureWelcomeHosts))
                        FirstPartyUrlPolicy.SetServerProvided(entry.FirstPartyHosts, entry.PlaybackProxyPaths);

                    // v3.1: server's negotiated format is per-connection,
                    // never cached — read from the dynamic fields the
                    // welcome_cached frame carries. Null / missing field
                    // means json (v3.0 server, or v3.1 server we sent
                    // json-only opt-out to).
                    _negotiatedFormat = cached?.NegotiatedFormat ?? WireConstants.FormatJson;
                    _isMsgpackFormat = string.Equals(_negotiatedFormat, WireConstants.FormatMsgpack, StringComparison.Ordinal);

                    Logger.WriteFileOnly("[mesh][v3] welcome_cached hit node="
                        + (_serverNode ?? "?") + " v=" + negotiated
                        + " negotiated_format=" + _negotiatedFormat
                        + " features=" + (entry.Features != null
                            ? string.Join(",", entry.Features) : "<none>"));
                    // No INFO console line — equivalent state was already
                    // cached; the user doesn't need a "still v3, still
                    // connected" reminder. The connect+welcome banner
                    // already fired the first time.

                    // _welcomeTcs awaits a WelcomeFrame? — null is fine
                    // here. ResolveAsync waiters key off _serverProtocolVersion
                    // and _serverFeatures, both of which are now set.
                    _welcomeTcs?.TrySetResult(null);
                    doc.Dispose();
                    return;
                }
            case WireConstants.ActionResolveLog:
                LogResolveLogFrame(doc.RootElement);
                doc.Dispose();
                return;
            case WireConstants.ActionRateLimited:
                {
                    // Server refused an action over budget. Before this handler the
                    // frame was default-discarded, the pending TCS timed out into
                    // server_unreachable (retryable AND health-gate-unhealthy) and
                    // the retry loop added MORE load. rate_limited is non-retryable
                    // and healthy by construction: the server answered.
                    string meshAction = "";
                    if (doc.RootElement.TryGetProperty(WireConstants.FieldMeshAction, out var maEl)
                        && maEl.ValueKind == JsonValueKind.String)
                        meshAction = maEl.GetString() ?? "";
                    string? limitedId = null;
                    if (doc.RootElement.TryGetProperty("id", out var rlIdEl) && rlIdEl.ValueKind == JsonValueKind.String)
                        limitedId = rlIdEl.GetString();
                    int retryAfterSeconds = 0;
                    if (doc.RootElement.TryGetProperty(WireConstants.FieldRetryAfterSeconds, out var raEl)
                        && raEl.ValueKind == JsonValueKind.Number)
                        raEl.TryGetInt32(out retryAfterSeconds);
                    doc.Dispose();

                    ConsoleUx.Warn(LogComponent.Mesh, "server rate-limited "
                        + LogUtil.SanitizeForConsole(meshAction, 32)
                        + " -- pausing for " + Math.Clamp(retryAfterSeconds, 1, MaxRateLimitCooldownSeconds) + "s");

                    if (meshAction == WireConstants.ActionResolve)
                    {
                        int cooldown = Math.Clamp(retryAfterSeconds, 1, MaxRateLimitCooldownSeconds);
                        Interlocked.Exchange(ref _resolveRateLimitedUntilTicks,
                            DateTime.UtcNow.AddSeconds(cooldown).Ticks);
                        if (!string.IsNullOrEmpty(limitedId))
                        {
                            _inflightCids.TryRemove(limitedId!, out _);
                            if (_pending.TryRemove(limitedId!, out var limitedTcs))
                                limitedTcs.TrySetResult(MakeFallbackResult(limitedId!, WireConstants.FallbackRateLimited));
                        }
                        else
                        {
                            // Old servers omit the id; everything pending is a resolve
                            // and every one of them was just refused.
                            FailAllPending(WireConstants.FallbackRateLimited);
                        }
                    }
                    return;
                }
            case WireConstants.ActionProtocolError:
                {
                    // Server rejected a frame shape. Fail the named request fast
                    // (non-retryable, health-gate-neutral); without an id this may
                    // concern a fire-and-forget frame (playback_feedback), so only
                    // log -- never nuke unrelated pending resolves.
                    string peReason = "";
                    if (doc.RootElement.TryGetProperty("reason", out var peReasonEl)
                        && peReasonEl.ValueKind == JsonValueKind.String)
                        peReason = peReasonEl.GetString() ?? "";
                    string peField = "";
                    if (doc.RootElement.TryGetProperty("field", out var peFieldEl)
                        && peFieldEl.ValueKind == JsonValueKind.String)
                        peField = peFieldEl.GetString() ?? "";
                    string? peId = null;
                    if (doc.RootElement.TryGetProperty("id", out var peIdEl) && peIdEl.ValueKind == JsonValueKind.String)
                        peId = peIdEl.GetString();
                    doc.Dispose();

                    ConsoleUx.Warn(LogComponent.Mesh, "server protocol_error reason="
                        + LogUtil.SanitizeForConsole(peReason, 48)
                        + (peField.Length > 0 ? " field=" + LogUtil.SanitizeForConsole(peField, 48) : "")
                        + (peId != null ? " id=" + LogUtil.SanitizeForConsole(peId, 32) : ""));

                    if (!string.IsNullOrEmpty(peId))
                    {
                        _inflightCids.TryRemove(peId!, out _);
                        if (_pending.TryRemove(peId!, out var peTcs))
                            peTcs.TrySetResult(MakeFallbackResult(peId!, WireConstants.FallbackProtocolError));
                    }
                    return;
                }
            case WireConstants.ActionPong:
                _lastPongUtc = DateTime.UtcNow;
                doc.Dispose();
                return;
            case WireConstants.ActionPing:
                doc.Dispose();
                try
                {
                    // Snapshot _ws before send — same TOCTOU concern as the
                    // heartbeat loop's snapshot.
                    var pongWs = _ws;
                    if (pongWs is { State: WebSocketState.Open })
                    {
                        await SendTextFrameAsync(PongFrame, ct).ConfigureAwait(false);
                    }
                }
                catch { /* heartbeat will catch dead socket */ }
                return;
            default:
                // Server-supplied string — strip control chars + truncate so a
                // hostile or buggy server can't inject ANSI escapes into the
                // user's console window.
                ConsoleUx.Warn(LogComponent.Mesh, "unknown action -- discarding: "
                    + LogUtil.SanitizeForConsole(action, 64));
                doc.Dispose();
                return;
        }
    }


    private static void LogResolveLogFrame(JsonElement root)
    {
        string id = "";
        if (root.TryGetProperty("id", out var idEl) && idEl.ValueKind == JsonValueKind.String)
            id = idEl.GetString() ?? "";

        string level = "info";
        if (root.TryGetProperty("level", out var levelEl) && levelEl.ValueKind == JsonValueKind.String)
            level = levelEl.GetString() ?? "info";

        string message = "";
        if (root.TryGetProperty("message", out var msgEl) && msgEl.ValueKind == JsonValueKind.String)
            message = msgEl.GetString() ?? "";

        Logger.WriteFileOnly(
            "[mesh][resolve_log] id=" + LogUtil.SanitizeForConsole(id, 32) +
            " level=" + LogUtil.SanitizeForConsole(level, 16) +
            " " + LogUtil.SanitizeForConsole(message, 240));
    }



    // Dedupe parse-failure logs by exception type so a flapping server can't
    // fill scrollback. One log per (type) per minute, plus a counter.
    private readonly Dictionary<string, (DateTime LastEmit, int Count)> _parseFailDedupe = new();
    private void LogParseFailure(Exception ex, byte[] payload)
    {
        string key = ex.GetType().Name;
        var now = DateTime.UtcNow;
        bool emit;
        int count;
        lock (_parseFailDedupe)
        {
            if (!_parseFailDedupe.TryGetValue(key, out var entry)
                || (now - entry.LastEmit).TotalMinutes >= 1)
            {
                count = entry.Count + 1;
                _parseFailDedupe[key] = (now, count);
                emit = true;
            }
            else
            {
                count = entry.Count + 1;
                _parseFailDedupe[key] = (entry.LastEmit, count);
                emit = false;
            }
        }
        if (emit)
        {
            ConsoleUx.Warn(
                LogComponent.Mesh,
                "frame parse failed (" + key + " x" + count + " in last min): " +
                LogUtil.SanitizeForConsole(ex.Message, 80) +
                " — preview=" + LogUtil.PayloadPreview(payload, 120));
        }
    }

}
