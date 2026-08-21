using MessagePack;
using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

// v3.1 hot-path msgpack contract tests. Three goals:
//
//   1. Round-trip every msgpack DTO. Field-by-field assert. Catches a
//      forgotten [Key(N)] or accidental property reorder.
//   2. PINNED-BYTES test on a known-shape MsgpackResolvedFrame. The
//      server's smoke harness emits 118 / 199 / 108 byte frames in its
//      tonight-smoke output; this test pins our local encoder against
//      the same shape so a property reorder fails CI rather than
//      silently corrupting on the wire.
//   3. Forward-compat: MessagePack array-layout includes count, so a
//      v3.1 client reading a hypothetical v3.5 frame with extra
//      trailing fields must NOT throw — extra values get skipped.
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
        // Wire-tag-fragility guard. The msgpack encoding of a
        // [Key(N)]-attributed class is a fixed-length array. Encode a
        // known instance and assert the array's first elements appear
        // in the expected positional order. A property reorder in
        // MeshMsgpackDtos.cs would silently flip the wire layout —
        // this test catches it.
        //
        // Probe payload uses tiny string values so we can hand-decode
        // the leading bytes without a msgpack library: msgpack array
        // header for fixarray length 12 is 0x9C; fixstrs are 0xa0+len
        // followed by the bytes.
        var src = new MsgpackResolvedFrame
        {
            Action = "R",  // 1 byte: 0xa1 0x52
            Id = "I",      // 0xa1 0x49
            Url = "U",     // 0xa1 0x55
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

        // Layout we expect (13-element fixarray):
        //   0x9D                fixarray, n=13
        //   0xa1 0x52           "R"     (Action,  index 0)
        //   0xa1 0x49           "I"     (Id,      index 1)
        //   0xa1 0x55           "U"     (Url,     index 2)
        //   0xc0 × 10           nil     (indexes 3..12)
        Assert.Equal(0x9D, mp[0]);                    // fixarray, n=13
        Assert.Equal(0xA1, mp[1]); Assert.Equal((byte)'R', mp[2]);  // [0] Action
        Assert.Equal(0xA1, mp[3]); Assert.Equal((byte)'I', mp[4]);  // [1] Id
        Assert.Equal(0xA1, mp[5]); Assert.Equal((byte)'U', mp[6]);  // [2] Url
        // [3..11] = 9 nil bytes (0xc0 each).
        Assert.Equal(0xC0, mp[7]);   // [3]  Engine
        Assert.Equal(0xC0, mp[8]);   // [4]  Config
        Assert.Equal(0xC0, mp[9]);   // [5]  Container
        Assert.Equal(0xC0, mp[10]);  // [6]  VideoCodec
        Assert.Equal(0xC0, mp[11]);  // [7]  AudioCodec
        Assert.Equal(0xC0, mp[12]);  // [8]  Protocol
        Assert.Equal(0xC0, mp[13]);  // [9]  AudioChannels
        Assert.Equal(0xC0, mp[14]);  // [10] BytesEstimate
        Assert.Equal(0xC0, mp[15]);  // [11] ExpiresAt
        Assert.Equal(0xC0, mp[16]);  // [12] ResolvedHeight
        Assert.Equal(17, mp.Length);  // fixarray header + 3 fixstrs (2B each) + 10 nils
    }

    [Fact]
    public void MsgpackFallbackNativeFrame_field_order_is_pinned()
    {
        // Same wire-tag-fragility guard for the 4-field DTO. Layout:
        //   0x94                fixarray, n=4
        //   0xa1 0x46           "F"  (Action, index 0)
        //   0xa1 0x49           "I"  (Id,     index 1)
        //   0xa1 0x52           "R"  (Reason, index 2)
        //   0xc0                nil  (PublicMessage, index 3)
        var src = new MsgpackFallbackNativeFrame
        {
            Action = "F",
            Id = "I",
            Reason = "R",
        };
        byte[] mp = MessagePackSerializer.Serialize(src);

        Assert.Equal(0x94, mp[0]);                      // fixarray, n=4
        Assert.Equal(0xA1, mp[1]); Assert.Equal((byte)'F', mp[2]);  // [0] Action
        Assert.Equal(0xA1, mp[3]); Assert.Equal((byte)'I', mp[4]);  // [1] Id
        Assert.Equal(0xA1, mp[5]); Assert.Equal((byte)'R', mp[6]);  // [2] Reason
        Assert.Equal(0xC0, mp[7]);                      // [3] PublicMessage
        Assert.Equal(8, mp.Length);
    }

    [Fact]
    public void MsgpackResolvedFrame_tolerates_extra_trailing_fields()
    {
        // Forward-compat test (per server protocol spec). MessagePack's
        // array layout includes the element count, so a v3.1 client
        // reading a hypothetical v3.5 frame with extra trailing fields
        // must silently skip the extras instead of throwing.
        //
        // Build via hand-modify of a real v3.1 encoding rather than
        // a separate writer setup: take MessagePackSerializer's output,
        // bump the fixarray header from n=12 (0x9C) to n=14 (0x9E),
        // append 2 extra elements (a fixstr-0 then a nil).
        var v31 = new MsgpackResolvedFrame
        {
            Action = "resolved",
            Id = "id1",
            Url = "https://example.com",
        };
        byte[] v31Bytes = MessagePackSerializer.Serialize(v31);

        var extended = new byte[v31Bytes.Length + 2];   // +1 fixstr-0, +1 nil
        v31Bytes.CopyTo(extended.AsSpan());
        extended[0] = 0x9F;                              // fixarray-15
        extended[v31Bytes.Length] = 0xA0;                // [13] fixstr-0 ("")
        extended[v31Bytes.Length + 1] = 0xC0;            // [14] nil

        // Per Q6 the extras are silently skipped.
        var round = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(extended);
        Assert.NotNull(round);
        Assert.Equal("resolved", round!.Action);
        Assert.Equal("id1", round.Id);
        Assert.Equal("https://example.com", round.Url);
    }

    [Fact]
    public void MsgpackResolvedFrame_tolerates_short_array_from_older_server()
    {
        // Old-server compat: a pre-resolved_height server emits a 12-element
        // resolved array. The 13-key DTO must leave the missing tail null
        // instead of throwing.
        var full = new MsgpackResolvedFrame
        {
            Action = "resolved",
            Id = "id1",
            Url = "https://example.com",
        };
        byte[] bytes = MessagePackSerializer.Serialize(full);
        // Shrink: drop the last element ([12] nil) and rewrite the header.
        var shortBytes = bytes.AsSpan(0, bytes.Length - 1).ToArray();
        shortBytes[0] = 0x9C;  // fixarray-12

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
        var shortBytes = bytes.AsSpan(0, bytes.Length - 1).ToArray();
        shortBytes[0] = 0x93;  // fixarray-3

        var round = MessagePackSerializer.Deserialize<MsgpackFallbackNativeFrame>(shortBytes);
        Assert.NotNull(round);
        Assert.Equal("warp_down", round!.Reason);
        Assert.Null(round.PublicMessage);
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
        // Real-world server emit: the v3.1 wire shape uses nil for
        // fields a particular resolved frame doesn't have (e.g. a
        // browser-extracted result without bytes_estimate). Confirm
        // the decoder doesn't trip on a frame where most fields are
        // nil. Mimics a typical fallback_native shape encoded as
        // resolved (defensive — server shouldn't actually do that).
        var src = new MsgpackResolvedFrame
        {
            Action = "resolved",
            Id = "shortid",
            Url = "https://x",
            // All v2 fields null.
        };
        byte[] mp = MessagePackSerializer.Serialize(src);
        var round = MessagePackSerializer.Deserialize<MsgpackResolvedFrame>(mp);
        Assert.Equal("resolved", round!.Action);
        Assert.Null(round.Engine);
        Assert.Null(round.AudioChannels);
        Assert.Null(round.BytesEstimate);
    }
}
