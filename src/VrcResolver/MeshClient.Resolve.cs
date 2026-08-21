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
    // Lossless forward: caller supplies the full ResolveRequest DTO from the
    // pipe. The watchdog adds protocol_version=2 if the server is v2-capable
    // and the patched yt-dlp didn't already set it; everything else round-trips
    // unchanged via [JsonExtensionData] on the DTO.
    //
    // Returns a MeshResolveResult carrying the verified raw response bytes
    // PLUS the parsed action and server-supplied reason. The caller writes
    // Frame straight to the pipe — no JsonDocument re-encode on the hot path.
    public async Task<MeshResolveResult> ResolveAsync(ResolveRequest req, CancellationToken ct)
    {
        // H5: defensive against null DTO from a misbehaving caller. Synthesize
        // a fallback rather than NRE before we have an id to key on.
        if (req == null)
            return MakeFallbackResult("", WireConstants.FallbackInternalError);

        // Generate per-attempt id if patched yt-dlp didn't supply one. Needed
        // for the pending-TCS key regardless.
        if (string.IsNullOrEmpty(req.Id))
            req.Id = Guid.NewGuid().ToString("N");

        var ws = _ws;
        if (ws is not { State: WebSocketState.Open })
            return MakeFallbackResult(req.Id, WireConstants.FallbackServerUnreachable);

        // Per-connection welcome handshake — wait up to 1s so we know whether
        // the server is v2-capable before deciding whether to opt into v2
        // response fields. After the first wait completes (welcome or 1s
        // fallback) the TCS stays completed for the connection's lifetime
        // and subsequent resolves return instantly.
        var welcomeTcs = _welcomeTcs;
        if (welcomeTcs is { Task.IsCompleted: false })
        {
            try { await welcomeTcs.Task.WaitAsync(ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { throw; }
        }

        // Stamp protocol_version=ClientProtocolVersion (currently 3) ONLY
        // when the server is v2-or-newer AND the patched yt-dlp has
        // signalled awareness of v2+ (either by setting protocol_version
        // itself, or populating any optional v2 request field). Pre-fix
        // this auto-stamped on any v1-shape request — pushing v2 response
        // fields onto a strict-shape v1 patched yt-dlp that never opted
        // in. Now: lossless v1 in → v1 out for callers that haven't
        // declared v2 awareness.
        //
        // Wire note: a v1-shape wrapper short-circuits CallerOptedIntoV2
        // and req.ProtocolVersion stays null. The default-options
        // JsonSerializer still emits `"protocol_version": null` on the
        // wire (no JsonIgnoreCondition.WhenWritingNull); server must
        // coalesce null → v1 — same behaviour since v2 first shipped.
        if (_serverProtocolVersion >= 2 && !req.ProtocolVersion.HasValue && CallerOptedIntoV2(req))
            req.ProtocolVersion = WireConstants.ClientProtocolVersion;

        var tcs = new TaskCompletionSource<MeshResolveResult>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[req.Id] = tcs;
        // Stash whichever cid the originating request carries (prefer
        // patched-yt-dlp-supplied correlation_id, else fall back to the
        // request id) so the resolved-frame handler can attach it to the
        // (url → cid) recent-resolves map for VrcLogMonitor.
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
                // Socket closed between the Open check above and the send.
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

    // Detect whether the patched yt-dlp populated any v2 request field. Used
    // to decide whether the watchdog should auto-stamp protocol_version=
    // ClientProtocolVersion (currently 3) — the audit's BC1 finding
    // flagged that a strict-shape v1 patched yt-dlp shouldn't suddenly
    // start receiving v2+ response fields it never opted into. v3 didn't
    // change this gate; the constant being stamped just bumped to 3.
    private static bool CallerOptedIntoV2(ResolveRequest req) =>
        req.ProtocolVersion.HasValue ||
        !string.IsNullOrEmpty(req.CorrelationId) ||
        req.AcceptProtocols != null ||
        req.AcceptCodecs != null ||
        req.MaxAudioChannels.HasValue ||
        !string.IsNullOrEmpty(req.VrchatFormatArg);

    // Server-emitted fallback_native — recorded for grep but no longer
    // surfaced on console. The user-facing per-resolve summary in
    // LocalIpcServer paints the same information as a single coloured
    // "!! fallback (<reason>)" line; this mesh-side trace is redundant
    // there and stayed visible only as legacy verbosity. Routed to the
    // rolling watchdog log so deep diagnosis still has the per-frame
    // record (with the v2-reason advisory copy preserved).
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
        // Clear inflight cids unconditionally — anything still pending now will
        // fail with no `resolved` to redeem the entry. Leaks in the dictionary
        // would slowly grow under sustained reconnect storms.
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
        // Per-id visibility for post-hoc correlation. Truncate the id list at
        // a reasonable size so a one-off disaster (50+ pending) doesn't fill
        // the scrollback in one line.
        const int MaxIdsInLine = 8;
        string idList = failedIds.Count <= MaxIdsInLine
            ? string.Join(",", failedIds)
            : string.Join(",", failedIds.GetRange(0, MaxIdsInLine)) + ",...(+" + (failedIds.Count - MaxIdsInLine) + ")";
        ConsoleUx.Warn(
            LogComponent.Mesh,
            "failing " + failedIds.Count + " pending requests reason=" + reason +
            " ids=" + idList);
    }

    // Synthesize a fallback_native frame (raw JSON bytes + parsed action +
    // reason) for callers that never made it onto the wire — null DTO,
    // socket down, send threw, mesh disconnect during outstanding wait, etc.
    // Bytes match what the server would emit for the same shape so the pipe
    // forward is wire-identical to a real server fallback.
    private static MeshResolveResult MakeFallbackResult(string id, string reason)
    {
        // AOT migration: anonymous-type SerializeToUtf8Bytes replaced
        // with a typed ResolveResponse populated with just the three
        // wire fields the synthetic fallback needs. Routed through the
        // MeshFallbackJsonContext (WhenWritingNull options) so the v2
        // nullable response fields stay omitted on the wire; v1
        // patched-yt-dlp consumers see byte-identical bytes to the
        // pre-migration shape.
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
