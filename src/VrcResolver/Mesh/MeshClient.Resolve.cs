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

        var ws = _ws;
        if (ws is not { State: WebSocketState.Open })
            return MakeFallbackResult(req.Id, WireConstants.FallbackServerUnreachable);

        var welcomeTcs = _welcomeTcs;
        if (welcomeTcs is { Task.IsCompleted: false })
        {
            try { await welcomeTcs.Task.WaitAsync(ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { throw; }
        }

        if (_serverProtocolVersion >= 2 && !req.ProtocolVersion.HasValue && CallerOptedIntoV2(req))
            req.ProtocolVersion = WireConstants.ClientProtocolVersion;

        var tcs = new TaskCompletionSource<MeshResolveResult>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[req.Id] = tcs;
        _inflightCids[req.Id] = string.IsNullOrEmpty(req.CorrelationId) ? req.Id : req.CorrelationId!;

        byte[] payload;
        try
        {
            payload = JsonSerializer.SerializeToUtf8Bytes(req, MeshJsonContext.Default.ResolveRequest);
        }
        catch (Exception ex)
        {
            _pending.TryRemove(req.Id, out _);
            _inflightCids.TryRemove(req.Id, out _);
            ConsoleUx.Warn(LogComponent.Mesh, $"request serialization failed id={req.Id}: {ex.Message}");
            return MakeFallbackResult(req.Id, WireConstants.FallbackInternalError);
        }

        try
        {
            if (!await SendTextFrameAsync(payload, ct).ConfigureAwait(false))
            {
                _pending.TryRemove(req.Id, out _);
                _inflightCids.TryRemove(req.Id, out _);
                return MakeFallbackResult(req.Id, WireConstants.FallbackServerUnreachable);
            }
        }
        catch (Exception ex)
        {
            _pending.TryRemove(req.Id, out _);
            _inflightCids.TryRemove(req.Id, out _);
            ConsoleUx.Warn(
                LogComponent.Mesh,
                "send failed id=" + req.Id +
                CidSuffix(req.CorrelationId) +
                ": " + ex.GetType().Name + ": " +
                LogUtil.SanitizeForConsole(ex.Message, 160));
            return MakeFallbackResult(req.Id, WireConstants.FallbackServerUnreachable);
        }

        string id = req.Id;
        await using var reg = ct.Register(() =>
        {
            if (_pending.TryRemove(id, out var t)) t.TrySetCanceled();
            _inflightCids.TryRemove(id, out _);
        });

        try
        {
            return await tcs.Task.ConfigureAwait(false);
        }
        catch (TaskCanceledException)
        {
            return MakeFallbackResult(id, WireConstants.FallbackServerUnreachable);
        }
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
