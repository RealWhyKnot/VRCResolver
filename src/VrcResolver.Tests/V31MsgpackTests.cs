using MessagePack;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class V31MsgpackTests
{
    [Fact]
    public void MsgpackResolvedFrame_round_trips()
    {
        var src = new MsgpackResolvedFrame
        {
            Action = WireConstants.ActionResolved,
            Id = "x9k2pq8r4mzj7vn3",
            Url = "https://rr1.googlevideo.com/foo",
            Engine = "yt-dlp",
            Config = "youtube-tv-combo",
            Container = "mp4",
            VideoCodec = "h264",
            AudioCodec = "aac",
            Protocol = "http",
            AudioChannels = 2,
            BytesEstimate = 12345678L,
            ExpiresAt = "2026-05-04T21:00:00Z",
        };
        byte[] mp = MessagePackSerializer.Serialize(src);
        var round = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(mp);

        Assert.NotNull(round);
        Assert.Equal(src.Action, round!.Action);
        Assert.Equal(src.Id, round.Id);
        Assert.Equal(src.Url, round.Url);
        Assert.Equal(src.Engine, round.Engine);
        Assert.Equal(src.Config, round.Config);
        Assert.Equal(src.Container, round.Container);
        Assert.Equal(src.VideoCodec, round.VideoCodec);
        Assert.Equal(src.AudioCodec, round.AudioCodec);
        Assert.Equal(src.Protocol, round.Protocol);
        Assert.Equal(src.AudioChannels, round.AudioChannels);
        Assert.Equal(src.BytesEstimate, round.BytesEstimate);
        Assert.Equal(src.ExpiresAt, round.ExpiresAt);
    }

    [Fact]
    public void MsgpackFallbackNativeFrame_round_trips()
    {
        var src = new MsgpackFallbackNativeFrame
        {
            Action = WireConstants.ActionFallbackNative,
            Id = "abc123",
            Reason = WireConstants.FallbackDiscoveryInProgress,
        };
        byte[] mp = MessagePackSerializer.Serialize(src);
        var round = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(mp);

        Assert.NotNull(round);
        Assert.Equal(src.Action, round!.Action);
        Assert.Equal(src.Id, round.Id);
        Assert.Equal(src.Reason, round.Reason);
    }

    [Fact]
    public void MsgpackResolveLogFrame_round_trips()
    {
        var src = new MsgpackResolveLogFrame
        {
            Action = WireConstants.ActionResolveLog,
            Id = "id1",
            Message = "trying youtube-tv-combo",
        };
        byte[] mp = MessagePackSerializer.Serialize(src);
        var round = MessagePackSerializer.Deserialize<MsgpackResolveLogFrame>(mp);

        Assert.NotNull(round);
        Assert.Equal(src.Action, round!.Action);
        Assert.Equal(src.Id, round.Id);
        Assert.Equal(src.Message, round.Message);
    }

    [Fact]
    public void MsgpackResolvedFrame_field_order_is_pinned()
    {
        var src = new MsgpackResolvedFrame
        {
            Action = "R",
            Id = "I",
            Url = "U",
            Engine = null,
            Config = null,
            Container = null,
            VideoCodec = null,
            AudioCodec = null,
            Protocol = null,
            AudioChannels = null,
            BytesEstimate = null,
            ExpiresAt = null,
            ResolvedHeight = null,
        };
        byte[] mp = MessagePackSerializer.Serialize(src);

        Assert.Equal(0x9D, mp[0]);
        Assert.Equal(0xA1, mp[1]); Assert.Equal((byte)'R', mp[2]);
        Assert.Equal(0xA1, mp[3]); Assert.Equal((byte)'I', mp[4]);
        Assert.Equal(0xA1, mp[5]); Assert.Equal((byte)'U', mp[6]);
        Assert.Equal(0xC0, mp[7]);
        Assert.Equal(0xC0, mp[8]);
        Assert.Equal(0xC0, mp[9]);
        Assert.Equal(0xC0, mp[10]);
        Assert.Equal(0xC0, mp[11]);
        Assert.Equal(0xC0, mp[12]);
        Assert.Equal(0xC0, mp[13]);
        Assert.Equal(0xC0, mp[14]);
        Assert.Equal(0xC0, mp[15]);
        Assert.Equal(0xC0, mp[16]);
        Assert.Equal(17, mp.Length);
    }

    [Fact]
    public void MsgpackFallbackNativeFrame_field_order_is_pinned()
    {
        var src = new MsgpackFallbackNativeFrame
        {
            Action = "F",
            Id = "I",
            Reason = "R",
        };
        byte[] mp = MessagePackSerializer.Serialize(src);

        Assert.Equal(0x95, mp[0]);
        Assert.Equal(0xA1, mp[1]); Assert.Equal((byte)'F', mp[2]);
        Assert.Equal(0xA1, mp[3]); Assert.Equal((byte)'I', mp[4]);
        Assert.Equal(0xA1, mp[5]); Assert.Equal((byte)'R', mp[6]);
        Assert.Equal(0xC0, mp[7]);
        Assert.Equal(0xC0, mp[8]);
        Assert.Equal(9, mp.Length);
    }

    [Fact]
    public void MsgpackResolvedFrame_tolerates_extra_trailing_fields()
    {
        var v31 = new MsgpackResolvedFrame
        {
            Action = "resolved",
            Id = "id1",
            Url = "https://example.com",
        };
        byte[] v31Bytes = MessagePackSerializer.Serialize(v31);

        var extended = new byte[v31Bytes.Length + 2];
        v31Bytes.CopyTo(extended.AsSpan());
        extended[0] = 0x9F;
        extended[v31Bytes.Length] = 0xA0;
        extended[v31Bytes.Length + 1] = 0xC0;

        var round = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(extended);
        Assert.NotNull(round);
        Assert.Equal("resolved", round!.Action);
        Assert.Equal("id1", round.Id);
        Assert.Equal("https://example.com", round.Url);
    }

    [Fact]
    public void MsgpackResolvedFrame_tolerates_short_array_from_older_server()
    {
        var full = new MsgpackResolvedFrame
        {
            Action = "resolved",
            Id = "id1",
            Url = "https://example.com",
        };
        byte[] bytes = MessagePackSerializer.Serialize(full);
        var shortBytes = bytes.AsSpan(0, bytes.Length - 1).ToArray();
        shortBytes[0] = 0x9C;

        var round = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(shortBytes);
        Assert.NotNull(round);
        Assert.Equal("id1", round!.Id);
        Assert.Null(round.ResolvedHeight);
    }

    [Fact]
    public void MsgpackFallbackNativeFrame_tolerates_short_array_from_older_server()
    {
        var full = new MsgpackFallbackNativeFrame
        {
            Action = "fallback_native",
            Id = "id1",
            Reason = "warp_down",
        };
        byte[] bytes = MessagePackSerializer.Serialize(full);
        var shortBytes = bytes.AsSpan(0, bytes.Length - 2).ToArray();
        shortBytes[0] = 0x93;

        var round = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(shortBytes);
        Assert.NotNull(round);
        Assert.Equal("warp_down", round!.Reason);
        Assert.Null(round.PublicMessage);
        Assert.Null(round.RetryAfterMs);
    }

    [Fact]
    public void MsgpackFallbackNativeFrame_tolerates_four_element_frame_without_retry_after()
    {
        byte[] mp =
        {
            0x94,
            0xA1, (byte)'F',
            0xA1, (byte)'I',
            0xA1, (byte)'R',
            0xC0,
        };
        var round = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(mp);
        Assert.NotNull(round);
        Assert.Equal("I", round!.Id);
        Assert.Null(round.RetryAfterMs);
    }

    [Fact]
    public void MsgpackFallbackNativeFrame_carries_retry_after()
    {
        var src = new MsgpackFallbackNativeFrame
        {
            Action = "fallback_native",
            Id = "id1",
            Reason = "discovery_in_progress",
            RetryAfterMs = 8500,
        };
        byte[] bytes = MessagePackSerializer.Serialize(src);
        var round = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(bytes);
        Assert.Equal(8500, round!.RetryAfterMs);
    }

    [Fact]
    public void MsgpackFallbackNativeFrame_carries_public_message()
    {
        var src = new MsgpackFallbackNativeFrame
        {
            Action = "fallback_native",
            Id = "id1",
            Reason = "drm_detected",
            PublicMessage = "Content is DRM-protected and can't be played.",
        };
        byte[] bytes = MessagePackSerializer.Serialize(src);
        var round = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(bytes);
        Assert.Equal("Content is DRM-protected and can't be played.", round!.PublicMessage);
    }

    [Fact]
    public void MsgpackResolvedFrame_decodes_partial_trailing_nil_fields()
    {
        var src = new MsgpackResolvedFrame
        {
            Action = "resolved",
            Id = "shortid",
            Url = "https://x",
        };
        byte[] mp = MessagePackSerializer.Serialize(src);
        var round = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(mp);
        Assert.Equal("resolved", round!.Action);
        Assert.Null(round.Engine);
        Assert.Null(round.AudioChannels);
        Assert.Null(round.BytesEstimate);
    }
}
