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
                    string id = "";
                    if (doc.RootElement.TryGetProperty("id", out var idEl) && idEl.ValueKind == JsonValueKind.String)
                        id = idEl.GetString() ?? "";

                    string? reason = null;
                    if (doc.RootElement.TryGetProperty("reason", out var reasonEl) && reasonEl.ValueKind == JsonValueKind.String)
                        reason = reasonEl.GetString();

                    int? retryAfterMs = null;
                    if (action == WireConstants.ActionFallbackNative
                        && doc.RootElement.TryGetProperty("retry_after_ms", out var retryEl)
                        && retryEl.ValueKind == JsonValueKind.Number
                        && retryEl.TryGetInt32(out int retryAfter))
                    {
                        retryAfterMs = retryAfter;
                    }

                    string? resolvedUrl = null;
                    if (action == WireConstants.ActionResolved
                        && doc.RootElement.TryGetProperty("url", out var urlEl)
                        && urlEl.ValueKind == JsonValueKind.String)
                    {
                        resolvedUrl = urlEl.GetString();
                    }

                    if (action == WireConstants.ActionResolved
                        && doc.RootElement.TryGetProperty("bytes_estimate", out var beEl)
                        && beEl.ValueKind == JsonValueKind.Number
                        && beEl.TryGetInt64(out long bytesEstimate))
                    {
                        WatchdogStats.RecordBytesEstimate(bytesEstimate);
                    }

                    if (action == WireConstants.ActionFallbackNative)
                        LogFallbackNative(id, reason);

                    doc.Dispose();

                    if (string.IsNullOrEmpty(id)) return;

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
                        tcs.TrySetResult(new MeshResolveResult(payload, action, reason, retryAfterMs));
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
                        Interlocked.CompareExchange(ref _serverProtocolVersion, 1, 0);
                    }

                    if (welcome != null)
                    {
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

                        _negotiatedFormat = welcome.NegotiatedFormat ?? WireConstants.FormatJson;
                        _isMsgpackFormat = string.Equals(_negotiatedFormat, WireConstants.FormatMsgpack, StringComparison.Ordinal);
                        Logger.WriteFileOnly("[mesh][v3.1] negotiated_format=" + _negotiatedFormat
                            + " isMsgpack=" + _isMsgpackFormat);

                        if (welcome.Features == null)
                            ConsoleUx.Warn(LogComponent.Mesh, "welcome missing required field: features");

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
                    if (!_isV3Connection)
                    {
                        ConsoleUx.Warn(LogComponent.Mesh, "welcome_cached received on non-v3 connection -- protocol error, reconnecting");
                        try { _ws?.Abort(); } catch { }
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
                        if (!string.IsNullOrEmpty(_currentNodeHost))
                            _welcomeCache.Invalidate(_currentNodeHost);
                        ConsoleUx.Warn(LogComponent.Mesh, "welcome_cached but local entry missing -- invalidating + reconnecting");
                        try { _ws?.Abort(); } catch { }
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

                    _negotiatedFormat = cached?.NegotiatedFormat ?? WireConstants.FormatJson;
                    _isMsgpackFormat = string.Equals(_negotiatedFormat, WireConstants.FormatMsgpack, StringComparison.Ordinal);

                    Logger.WriteFileOnly("[mesh][v3] welcome_cached hit node="
                        + (_serverNode ?? "?") + " v=" + negotiated
                        + " negotiated_format=" + _negotiatedFormat
                        + " features=" + (entry.Features != null
                            ? string.Join(",", entry.Features) : "<none>"));

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
                            FailAllPending(WireConstants.FallbackRateLimited);
                        }
                    }
                    return;
                }
            case WireConstants.ActionProtocolError:
                {
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
                    var pongWs = _ws;
                    if (pongWs is { State: WebSocketState.Open })
                    {
                        await SendTextFrameAsync(PongFrame, ct).ConfigureAwait(false);
                    }
                }
                catch { }
                return;
            default:
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
