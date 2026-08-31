using System.Net.WebSockets;
using System.Text.Json;
using MessagePack;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class MeshClient
{
    private async Task DispatchBinaryFrameAsync(byte[] payload, CancellationToken ct)
    {
        string action;
        try
        {
            var reader = new MessagePack.MessagePackReader(payload);
            int count = reader.ReadArrayHeader();
            if (count < 1)
            {
                LogBinaryParseFailure("empty msgpack array (count=0)", payload);
                return;
            }
            action = reader.ReadString() ?? "";
        }
        catch (Exception ex)
        {
            LogBinaryParseFailure("action peek: " + ex.GetType().Name + ": " + ex.Message, payload);
            return;
        }

        switch (action)
        {
            case WireConstants.ActionResolved:
                {
                    MsgpackResolvedFrame? mp;
                    try { mp = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(payload, s_msgpackOpts); }
                    catch (Exception ex)
                    {
                        LogBinaryParseFailure("resolved deserialize: " + ex.GetType().Name + ": " + ex.Message, payload);
                        return;
                    }
                    if (mp == null || string.IsNullOrEmpty(mp.Id)) return;

                    var resp = new ResolveResponse
                    {
                        Action = WireConstants.ActionResolved,
                        Id = mp.Id ?? "",
                        Url = mp.Url,
                        Engine = mp.Engine,
                        Config = mp.Config,
                        Container = mp.Container,
                        VideoCodec = mp.VideoCodec,
                        AudioCodec = mp.AudioCodec,
                        Protocol = mp.Protocol,
                        AudioChannels = mp.AudioChannels,
                        BytesEstimate = mp.BytesEstimate,
                        ExpiresAt = mp.ExpiresAt,
                        ResolvedHeight = mp.ResolvedHeight,
                    };
                    byte[] jsonFrame = JsonSerializer.SerializeToUtf8Bytes(resp, MeshJsonContext.Default.ResolveResponse);

                    if (mp.BytesEstimate.HasValue)
                        WatchdogStats.RecordBytesEstimate(mp.BytesEstimate.Value);

                    _inflightCids.TryRemove(mp.Id!, out var cid);
                    if (!string.IsNullOrEmpty(cid) && !string.IsNullOrEmpty(mp.Url))
                        RememberResolvedUrlCid(mp.Url!, cid);

                    if (_pending.TryRemove(mp.Id!, out var tcs))
                    {
                        _reconnectAttempt = 0;
                        tcs.TrySetResult(new MeshResolveResult(jsonFrame, WireConstants.ActionResolved, null));
                    }
                    return;
                }
            case WireConstants.ActionFallbackNative:
                {
                    MsgpackFallbackNativeFrame? mp;
                    try { mp = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(payload, s_msgpackOpts); }
                    catch (Exception ex)
                    {
                        LogBinaryParseFailure("fallback_native deserialize: " + ex.GetType().Name + ": " + ex.Message, payload);
                        return;
                    }
                    if (mp == null || string.IsNullOrEmpty(mp.Id)) return;

                    LogFallbackNative(mp.Id!, mp.Reason);

                    var resp = new ResolveResponse
                    {
                        Action = WireConstants.ActionFallbackNative,
                        Id = mp.Id ?? "",
                        Reason = mp.Reason,
                        PublicMessage = mp.PublicMessage,
                        RetryAfterMs = mp.RetryAfterMs,
                    };
                    byte[] jsonFrame = JsonSerializer.SerializeToUtf8Bytes(resp, MeshJsonContext.Default.ResolveResponse);

                    _inflightCids.TryRemove(mp.Id!, out _);
                    if (_pending.TryRemove(mp.Id!, out var tcs))
                    {
                        tcs.TrySetResult(new MeshResolveResult(jsonFrame, WireConstants.ActionFallbackNative, mp.Reason));
                    }
                    return;
                }
            case WireConstants.ActionResolveLog:
                {
                    MsgpackResolveLogFrame? mp;
                    try { mp = MessagePackSerializer.Deserialize<MsgpackResolveLogFrame>(payload, s_msgpackOpts); }
                    catch (Exception ex)
                    {
                        LogBinaryParseFailure("resolve_log deserialize: " + ex.GetType().Name + ": " + ex.Message, payload);
                        return;
                    }
                    if (mp == null) return;
                    Logger.WriteFileOnly(
                        "[mesh][resolve_log] id=" + LogUtil.SanitizeForConsole(mp.Id ?? "", 32) +
                        " " + LogUtil.SanitizeForConsole(mp.Message ?? "", 240));
                    return;
                }
            case WireConstants.ActionPong:
                _lastPongUtc = DateTime.UtcNow;
                Logger.WriteFileOnly("[mesh] pong received via binary path (server should send as Text per v3.1 spec)");
                return;
            case WireConstants.ActionPing:
                {
                    Logger.WriteFileOnly("[mesh] ping received via binary path (server should send as Text per v3.1 spec)");
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
                }
            case "protocol_error":
            case "rate_limited":
                ConsoleUx.Warn(LogComponent.Mesh, "" + action + " received via binary path "
                    + "(server should send as Text per v3.1 spec)");
                return;
            default:
                ConsoleUx.Warn(LogComponent.Mesh, "unknown binary action -- discarding: "
                    + LogUtil.SanitizeForConsole(action, 64));
                return;
        }
    }


    private void LogBinaryParseFailure(string detail, byte[] payload)
    {
        string key = "binary:" + detail.Split(':')[0];
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
                "binary frame parse failed (" + key + " x" + count + " in last min): " +
                LogUtil.SanitizeForConsole(detail, 200) +
                " — preview=" + LogUtil.PayloadPreview(payload, 60));
        }
    }

}
