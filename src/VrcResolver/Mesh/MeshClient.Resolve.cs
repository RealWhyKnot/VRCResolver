using System.Collections.Concurrent;
using System.Net.WebSockets;
using System.Reflection;
using System.Text.Json;
using MessagePack;
using MessagePack.Resolvers;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class MeshClient : IAsyncDisposable
{
    public async Task<MeshResolveResult> ResolveAsync(ResolveRequest req, CancellationToken ct)
    {
        if (req == null)
            return MakeFallbackResult("", WireConstants.FallbackInternalError);

        if (string.IsNullOrEmpty(req.Id))
            req.Id = Guid.NewGuid().ToString("N");

        if (Interlocked.Read(ref _resolveRateLimitedUntilTicks) > DateTime.UtcNow.Ticks)
            return MakeFallbackResult(req.Id, WireConstants.FallbackRateLimited);

        string id = req.Id;
        var tcs = new TaskCompletionSource<MeshResolveResult>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;
        _inflightCids[id] = string.IsNullOrEmpty(req.CorrelationId) ? id : req.CorrelationId!;
        await using var reg = ct.Register(() =>
        {
            if (_pending.TryRemove(id, out var t)) t.TrySetCanceled();
            _inflightCids.TryRemove(id, out _);
        });

        try
        {
            int sends = 0;
            while (true)
            {
                var link = await WaitForLinkAsync(tcs.Task, ct).ConfigureAwait(false);
                if (tcs.Task.IsCompleted) return await tcs.Task.ConfigureAwait(false);
                if (link == null)
                {
                    ForgetPending(id);
                    if (sends > 0)
                        Logger.WriteFileOnly("[mesh] no reconnect within " + _reconnectGrace.TotalSeconds
                            + "s; failing id=" + id + CidSuffix(req.CorrelationId));
                    return MakeFallbackResult(id, WireConstants.FallbackServerUnreachable);
                }

                if (_serverProtocolVersion >= 2 && !req.ProtocolVersion.HasValue && CallerOptedIntoV2(req))
                    req.ProtocolVersion = WireConstants.ClientProtocolVersion;

                byte[] payload;
                try
                {
                    payload = JsonSerializer.SerializeToUtf8Bytes(req, MeshJsonContext.Default.ResolveRequest);
                }
                catch (Exception ex)
                {
                    ForgetPending(id);
                    ConsoleUx.Warn(LogComponent.Mesh, $"request serialization failed id={id}: {ex.Message}");
                    return MakeFallbackResult(id, WireConstants.FallbackInternalError);
                }

                bool sent;
                try
                {
                    sent = await SendTextFrameAsync(payload, ct).ConfigureAwait(false);
                }
                catch (OperationCanceledException) when (ct.IsCancellationRequested) { throw; }
                catch (Exception ex)
                {
                    ConsoleUx.Warn(
                        LogComponent.Mesh,
                        "send failed id=" + id +
                        CidSuffix(req.CorrelationId) +
                        ": " + ex.GetType().Name + ": " +
                        LogUtil.SanitizeForConsole(ex.Message, 160));
                    sent = false;
                }

                if (!sent)
                {
                    using var waitCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
                    var giveUp = Task.Delay(_reconnectGrace, waitCts.Token);
                    var woke = await Task.WhenAny(link.Down.Task, tcs.Task, giveUp).ConfigureAwait(false);
                    waitCts.Cancel();
                    if (woke == giveUp && !tcs.Task.IsCompleted)
                    {
                        ForgetPending(id);
                        return MakeFallbackResult(id, WireConstants.FallbackServerUnreachable);
                    }
                    continue;
                }

                sends++;
                if (sends > 1)
                    Logger.WriteFileOnly("[mesh] resent id=" + id + CidSuffix(req.CorrelationId)
                        + " after reconnect (send " + sends + ")");

                var done = await Task.WhenAny(tcs.Task, link.Down.Task).ConfigureAwait(false);
                if (done == tcs.Task) return await tcs.Task.ConfigureAwait(false);
                Logger.WriteFileOnly("[mesh] connection dropped with id=" + id + CidSuffix(req.CorrelationId)
                    + " pending; resending after reconnect");
            }
        }
        catch (OperationCanceledException)
        {
            ForgetPending(id);
            return MakeFallbackResult(id, WireConstants.FallbackClientDeadlineExceeded);
        }
    }

    private void ForgetPending(string id)
    {
        _pending.TryRemove(id, out _);
        _inflightCids.TryRemove(id, out _);
    }

    internal static bool CallerOptedIntoV2(ResolveRequest req) =>
        req.ProtocolVersion.HasValue ||
        !string.IsNullOrEmpty(req.CorrelationId) ||
        req.AcceptProtocols != null ||
        req.AcceptCodecs != null ||
        req.MaxAudioChannels.HasValue ||
        req.PreferHighest.HasValue ||
        !string.IsNullOrEmpty(req.VrchatFormatArg);

    private static void LogFallbackNative(string id, string? reasonRaw)
    {
        string reason = LogUtil.SanitizeForConsole(reasonRaw ?? "", 64);

        string line = reason switch
        {
            WireConstants.ReasonUnityUnsupportedFormat =>
                $"[mesh] fallback_native id={id} reason=unity_unsupported_format (no Unity-playable stream — try AVPro)",
            WireConstants.ReasonWarpDown =>
                $"[mesh] fallback_native id={id} reason=warp_down (server WARP egress unhealthy — transient, retry shortly or another node)",
            _ =>
                $"[mesh] fallback_native id={id} reason={(string.IsNullOrEmpty(reason) ? "?" : reason)}",
        };
        Logger.WriteFileOnly(line);
    }

    private void FailAllPending(string reason)
    {
        _inflightCids.Clear();
        var failedIds = new List<string>();
        foreach (var kvp in _pending.ToArray())
        {
            if (_pending.TryRemove(kvp.Key, out var tcs))
            {
                failedIds.Add(kvp.Key);
                tcs.TrySetResult(MakeFallbackResult(kvp.Key, reason));
            }
        }

        if (failedIds.Count == 0) return;
        const int MaxIdsInLine = 8;
        string idList = failedIds.Count <= MaxIdsInLine
            ? string.Join(",", failedIds)
            : string.Join(",", failedIds.GetRange(0, MaxIdsInLine)) + ",...(+" + (failedIds.Count - MaxIdsInLine) + ")";
        ConsoleUx.Warn(
            LogComponent.Mesh,
            "failing " + failedIds.Count + " pending requests reason=" + reason +
            " ids=" + idList);
    }

    private static MeshResolveResult MakeFallbackResult(string id, string reason)
    {
        var frame = new ResolveResponse
        {
            Action = WireConstants.ActionFallbackNative,
            Id = id,
            Reason = reason,
        };
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(frame, MeshFallbackJsonContext.Default.ResolveResponse);
        return new MeshResolveResult(bytes, WireConstants.ActionFallbackNative, reason);
    }
}
