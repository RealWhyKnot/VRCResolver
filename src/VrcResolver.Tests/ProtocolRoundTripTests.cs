using System.Text.Json;
using VrcResolver;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

// Verifies the wire-protocol DTOs deserialize and re-serialize without
// losing v2 fields or unknown extension data. Regressions here would
// silently break v3+ forward-compat — invisible until a server-side
// field gets dropped on the floor.
public class ProtocolRoundTripTests
{
    [Fact]
    public void ResolveRequest_v2_fields_round_trip()
    {
        const string json = """
        {
          "action": "resolve",
          "id": "abc123",
          "url": "https://www.youtube.com/watch?v=foo",
          "player": "unity",
          "maxHeight": 1080,
          "protocol_version": 2,
          "correlation_id": "user-42",
          "accept_protocols": ["http"],
          "accept_codecs": ["h264", "aac"],
          "max_audio_channels": 2,
          "vrchat_format_arg": "bestvideo[height<=1080][vcodec^=avc1]+bestaudio/best"
        }
        """;
        var req = JsonSerializer.Deserialize<ResolveRequest>(json);
        Assert.NotNull(req);
        Assert.Equal("resolve", req!.Action);
        Assert.Equal("abc123", req.Id);
        Assert.Equal("unity", req.Player);
        Assert.Equal(1080, req.MaxHeight);
        Assert.Equal(2, req.ProtocolVersion);
        Assert.Equal("user-42", req.CorrelationId);
        Assert.Equal(new[] { "http" }, req.AcceptProtocols);
        Assert.Equal(new[] { "h264", "aac" }, req.AcceptCodecs);
        Assert.Equal(2, req.MaxAudioChannels);
        Assert.Equal("bestvideo[height<=1080][vcodec^=avc1]+bestaudio/best", req.VrchatFormatArg);
    }

    [Fact]
    public void ResolveRequest_unknown_fields_round_trip_via_extension_data()
    {
        const string json = """
        {
          "action": "resolve",
          "id": "abc",
          "url": "https://x",
          "player": "avpro",
          "maxHeight": 1080,
          "future_v3_field": {"nested": true, "values": [1,2,3]},
          "another_unknown": "preserve me"
        }
        """;
        var req = JsonSerializer.Deserialize<ResolveRequest>(json);
        Assert.NotNull(req);
        Assert.NotNull(req!.Extra);
        Assert.True(req.Extra!.ContainsKey("future_v3_field"));
        Assert.True(req.Extra.ContainsKey("another_unknown"));

        // Re-serialize and verify the unknowns survive the round trip.
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(req);
        using var doc = JsonDocument.Parse(bytes);
        Assert.True(doc.RootElement.TryGetProperty("future_v3_field", out var fv3));
        Assert.Equal(JsonValueKind.Object, fv3.ValueKind);
        Assert.True(fv3.TryGetProperty("nested", out var nested));
        Assert.True(nested.GetBoolean());
        Assert.True(doc.RootElement.TryGetProperty("another_unknown", out var au));
        Assert.Equal("preserve me", au.GetString());
    }

    [Fact]
    public void ResolveResponse_v2_fields_round_trip()
    {
        const string json = """
        {
          "action": "resolved",
          "id": "abc",
          "url": "https://signed-cdn/...",
          "engine": "yt-dlp",
          "config": "default",
          "container": "mp4",
          "video_codec": "h264",
          "audio_codec": "aac",
          "protocol": "http",
          "audio_channels": 2,
          "bytes_estimate": 12345678,
          "expires_at": "2026-01-01T00:00:00Z"
        }
        """;
        var resp = JsonSerializer.Deserialize<ResolveResponse>(json);
        Assert.NotNull(resp);
        Assert.Equal("mp4", resp!.Container);
        Assert.Equal("h264", resp.VideoCodec);
        Assert.Equal("aac", resp.AudioCodec);
        Assert.Equal("http", resp.Protocol);
        Assert.Equal(2, resp.AudioChannels);
        Assert.Equal(12345678L, resp.BytesEstimate);
        Assert.Equal("2026-01-01T00:00:00Z", resp.ExpiresAt);
    }

    [Fact]
    public void WelcomeFrame_round_trips_with_all_fields_and_extras()
    {
        const string json = """
        {
          "action": "welcome",
          "protocol_version": 2,
          "node": "node1",
          "warp_active": true,
          "engines": ["yt-dlp", "streamlink", "passthrough"],
          "features": ["unity_format_selection","accept_protocols","accept_codecs"],
          "yt_dlp_version": "2026.10.31",
          "server_version": "2026.5.4-canary",
          "future_capability": "respect"
        }
        """;
        var w = JsonSerializer.Deserialize<WelcomeFrame>(json);
        Assert.NotNull(w);
        Assert.Equal(2, w!.ProtocolVersion);
        Assert.Equal("node1", w.Node);
        Assert.True(w.WarpActive);
        Assert.Equal(3, w.Engines!.Length);
        Assert.Equal(3, w.Features!.Length);
        Assert.Equal("2026.10.31", w.YtDlpVersion);
        Assert.Equal("2026.5.4-canary", w.ServerVersion);
        Assert.NotNull(w.Extra);
        Assert.True(w.Extra!.ContainsKey("future_capability"));
    }

    [Fact]
    public void PlaybackFeedback_frame_matches_server_contract()
    {
        // Server-side validator (the server's mesh dispatcher) expects exactly:
        //   action="playback_feedback", url, kind, timestamp (ISO-8601 UTC),
        //   ms_since_open. Optional: correlation_id, client_id. The earlier
        //   R2 stash sent `timestamp_utc` which the server rejected with a
        //   protocol_error (field=timestamp); this test pins the new shape.
        var ts = new DateTime(2026, 5, 4, 7, 22, 0, DateTimeKind.Utc);
        byte[] bytes = MeshClient.BuildPlaybackFeedbackPayload(
            url: "https://us1.vrcresolver.com/r/abc",
            kind: WireConstants.PlaybackFeedbackLoadFailure,
            msSinceOpen: 4321,
            clientId: "client-xyz",
            correlationId: "cid-42",
            timestampUtc: ts);

        using var doc = JsonDocument.Parse(bytes);
        var root = doc.RootElement;
        Assert.Equal("playback_feedback", root.GetProperty("action").GetString());
        Assert.Equal("https://us1.vrcresolver.com/r/abc", root.GetProperty("url").GetString());
        Assert.Equal("load_failure", root.GetProperty("kind").GetString());
        Assert.Equal("2026-05-04T07:22:00.0000000Z", root.GetProperty("timestamp").GetString());
        Assert.Equal(4321, root.GetProperty("ms_since_open").GetInt32());
        Assert.Equal("client-xyz", root.GetProperty("client_id").GetString());
        Assert.Equal("cid-42", root.GetProperty("correlation_id").GetString());

        // Old field MUST NOT appear — server rejects with protocol_error.
        Assert.False(root.TryGetProperty("timestamp_utc", out _));
    }

    [Fact]
    public void PlaybackFeedback_frame_omits_correlation_id_when_unknown()
    {
        // When VrcLogMonitor sees an Opening for a URL not in the recent-
        // resolves cache (e.g. wrapper served via fallback_native, or pre-
        // existing AVPro URL from before watchdog start), correlation_id is
        // null. Frame must OMIT the field, not serialize "null", so the
        // server's missing-field semantics apply (skip cache lookup, fall
        // back to URL-host extraction).
        byte[] bytes = MeshClient.BuildPlaybackFeedbackPayload(
            url: "https://example.com/video.m3u8",
            kind: WireConstants.PlaybackFeedbackSilentStall,
            msSinceOpen: 12000,
            clientId: "client-xyz",
            correlationId: null,
            timestampUtc: DateTime.UtcNow);

        using var doc = JsonDocument.Parse(bytes);
        Assert.False(doc.RootElement.TryGetProperty("correlation_id", out _));
        Assert.True(doc.RootElement.TryGetProperty("client_id", out _));
        Assert.Equal("silent_stall", doc.RootElement.GetProperty("kind").GetString());
    }

    [Fact]
    public void PlaybackFeedback_frame_includes_playing_delivered_height()
    {
        byte[] bytes = MeshClient.BuildPlaybackFeedbackPayload(
            url: "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            kind: WireConstants.PlaybackFeedbackPlaying,
            msSinceOpen: 15000,
            clientId: "client-xyz",
            correlationId: "cid-42",
            timestampUtc: new DateTime(2026, 5, 11, 3, 30, 0, DateTimeKind.Utc),
            deliveredHeight: 720);

        string json = System.Text.Encoding.UTF8.GetString(bytes);
        Assert.Contains("\"kind\":\"playing\"", json);
        Assert.Contains("\"delivered_height\":720", json);

        using var doc = JsonDocument.Parse(bytes);
        Assert.Equal(720, doc.RootElement.GetProperty("delivered_height").GetInt32());
        Assert.Equal("playing", doc.RootElement.GetProperty("kind").GetString());
    }

    [Fact]
    public void PlaybackFeedback_v2_detail_is_carried_and_omitted_when_null()
    {
        byte[] withDetail = MeshClient.BuildPlaybackFeedbackPayload(
            url: "https://example.com/video",
            kind: WireConstants.PlaybackFeedbackResolvedRejected,
            msSinceOpen: 0,
            clientId: "client-xyz",
            correlationId: "cid-42",
            timestampUtc: DateTime.UtcNow,
            detail: WireConstants.OgFallbackReasonResolvedUrlRejected);
        using (var doc = JsonDocument.Parse(withDetail))
        {
            Assert.Equal("resolved_rejected", doc.RootElement.GetProperty("kind").GetString());
            Assert.Equal("resolved_url_rejected", doc.RootElement.GetProperty("detail").GetString());
        }

        byte[] withoutDetail = MeshClient.BuildPlaybackFeedbackPayload(
            url: "https://example.com/video",
            kind: WireConstants.PlaybackFeedbackCachePlay,
            msSinceOpen: 0,
            clientId: "client-xyz",
            correlationId: "cid-42",
            timestampUtc: DateTime.UtcNow);
        using (var doc = JsonDocument.Parse(withoutDetail))
        {
            Assert.Equal("cache_play", doc.RootElement.GetProperty("kind").GetString());
            Assert.False(doc.RootElement.TryGetProperty("detail", out _));
        }
    }

    [Fact]
    public void ResolveRequest_wrapper_deadline_ms_round_trips()
    {
        const string json = """
        {
          "action": "resolve",
          "id": "abc",
          "url": "https://x",
          "player": "avpro",
          "wrapper_deadline_ms": 14250
        }
        """;
        var req = JsonSerializer.Deserialize<ResolveRequest>(json);
        Assert.NotNull(req);
        Assert.Equal(14250, req!.WrapperDeadlineMs);

        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(req);
        using var doc = JsonDocument.Parse(bytes);
        Assert.True(doc.RootElement.TryGetProperty("wrapper_deadline_ms", out var prop));
        Assert.Equal(14250, prop.GetInt32());
    }

    [Fact]
    public void ResolveRequest_wrapper_deadline_ms_omitted_when_null()
    {
        var req = new ResolveRequest { Action = "resolve", Id = "x", Url = "https://x", Player = "avpro" };
        byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(req);
        using var doc = JsonDocument.Parse(bytes);
        Assert.False(doc.RootElement.TryGetProperty("wrapper_deadline_ms", out _));
    }

    [Fact]
    public void WireConstants_match_server_spec_strings()
    {
        // These are the constants the server-side audit explicitly listed
        // as needing to be byte-exact. A typo in either side breaks the
        // wire silently — easy unit-test guard.
        Assert.Equal("welcome", WireConstants.ActionWelcome);
        Assert.Equal("protocol_version", WireConstants.FieldProtocolVersion);
        Assert.Equal("accept_protocols", WireConstants.FieldAcceptProtocols);
        Assert.Equal("accept_codecs", WireConstants.FieldAcceptCodecs);
        Assert.Equal("max_audio_channels", WireConstants.FieldMaxAudioChannels);
        Assert.Equal("vrchat_format_arg", WireConstants.FieldVrchatFormatArg);
        Assert.Equal("correlation_id", WireConstants.FieldCorrelationId);
        Assert.Equal("delivered_height", WireConstants.FieldDeliveredHeight);
        Assert.Equal("unity_unsupported_format", WireConstants.ReasonUnityUnsupportedFormat);
        Assert.Equal("warp_down", WireConstants.ReasonWarpDown);
        Assert.Equal("playing", WireConstants.PlaybackFeedbackPlaying);
        Assert.Equal("avpro", WireConstants.PlayerAvPro);
        Assert.Equal("unity", WireConstants.PlayerUnity);
        // Bumped 2 → 3 alongside v3 wire-protocol shipping. v2 servers
        // negotiate down via Math.Clamp(server, 1, client) on welcome
        // receipt — backward compat preserved.
        Assert.Equal(3, WireConstants.ClientProtocolVersion);
    }

}
