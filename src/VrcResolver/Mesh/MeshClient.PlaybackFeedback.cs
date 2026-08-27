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
    public async Task SendPlaybackFeedbackAsync(string url, string kind, int msSinceOpen, int? deliveredHeight = null, string? detail = null, string? correlationIdOverride = null)
    {
        var ws = _ws;
        if (ws is not { State: WebSocketState.Open }) return;
        if (string.IsNullOrEmpty(url) || string.IsNullOrEmpty(kind)) return;

        var features = _serverFeatures;
        if (!HasFeature(features, WireConstants.ActionPlaybackFeedback))
            return;

        bool isV2Kind = kind == WireConstants.PlaybackFeedbackResolvedRejected
            || kind == WireConstants.PlaybackFeedbackOgFailed
            || kind == WireConstants.PlaybackFeedbackCachePlay;
        if (isV2Kind && !HasFeature(features, WireConstants.FeaturePlaybackFeedbackV2))
            return;
        if (!HasFeature(features, WireConstants.FeaturePlaybackFeedbackV2))
            detail = null;

        string? cid = correlationIdOverride ?? LookupRecentCorrelationId(url);

        byte[] payload;
        try
        {
            payload = BuildPlaybackFeedbackPayload(
                url, kind, msSinceOpen, _clientId, cid, DateTime.UtcNow, deliveredHeight, detail);
        }
        catch { return; }

        try
        {
            await SendTextFrameAsync(payload, CancellationToken.None).ConfigureAwait(false);
        }
        catch { }
    }

    internal static byte[] BuildPlaybackFeedbackPayload(
        string url,
        string kind,
        int msSinceOpen,
        string clientId,
        string? correlationId,
        DateTime timestampUtc,
        int? deliveredHeight = null,
        string? detail = null)
    {
        var frame = new PlaybackFeedbackFrame
        {
            Url = url,
            Kind = kind,
            Timestamp = timestampUtc.ToString("o"),
            MsSinceOpen = msSinceOpen,
            ClientId = clientId,
            CorrelationId = string.IsNullOrEmpty(correlationId) ? null : correlationId,
            DeliveredHeight = deliveredHeight is > 0 ? deliveredHeight : null,
            Detail = string.IsNullOrEmpty(detail) ? null : detail,
        };
        return JsonSerializer.SerializeToUtf8Bytes(frame, MeshJsonContext.Default.PlaybackFeedbackFrame);
    }

    private string? LookupRecentCorrelationId(string url)
    {
        lock (_recentCidsLock)
        {
            if (!_recentCids.TryGetValue(url, out var entry))
                return null;
            if (DateTime.UtcNow - entry.At > RecentCidsTtl)
            {
                _recentCids.Remove(url);
                return null;
            }
            return entry.Cid;
        }
    }

    private void RememberResolvedUrlCid(string url, string cid)
    {
        if (string.IsNullOrEmpty(url) || string.IsNullOrEmpty(cid)) return;
        lock (_recentCidsLock)
        {
            _recentCids[url] = (cid, DateTime.UtcNow);
            if (_recentCids.Count <= MaxRecentCids) return;

            string? oldestKey = null;
            DateTime oldestAt = DateTime.MaxValue;
            foreach (var kvp in _recentCids)
            {
                if (kvp.Value.At < oldestAt) { oldestAt = kvp.Value.At; oldestKey = kvp.Key; }
            }
            if (oldestKey != null) _recentCids.Remove(oldestKey);
        }
    }
}
