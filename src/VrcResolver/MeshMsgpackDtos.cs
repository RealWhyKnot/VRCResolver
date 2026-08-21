using MessagePack;

namespace VrcResolver;

// Watchdog-internal DTOs for the v3.1 msgpack hot-path decoder. NOT in
// VrcResolver.Shared because the wrapper (VrcResolver.YtDlp, AOT'd) MUST
// NOT pull a MessagePack dependency — the wrapper consumes JSON over
// the named pipe regardless of what wire format the watchdog
// negotiated with the server. Keeping these types watchdog-internal
// guarantees the wrapper csproj never sees a MessagePack assembly
// reference + the wrapper's AOT publish stays unaffected.
//
// Wire-tag-fragility rule (per server protocol spec):
//
//   * Integer keys [Key(N)] are positional. The N values MUST mirror
//     the server's MeshResolveProtocol.cs byte-exact.
//   * NEW fields go at the next free index, never reuse a removed
//     index, never reorder existing properties. A property reorder =
//     silent corruption on the wire (server emits N=2 thinking it's
//     Url, client reads N=2 thinking it's Engine -- same type, no
//     deserialize error, just wrong values).
//   * Trailing nullable fields beyond what THIS client knows are
//     skipped silently -- MessagePack array-layout includes the count
//     so a v3.1 client reading a v3.5 frame ignores the extra trailing
//     values without throwing.
//
// Field orders pinned per server protocol revision 2026-05-04. Update
// MsgpackContractTests.cs's pinned-bytes assertions if these change.

// "resolved" frame on the v3.1 hot path. Field order:
//   0:Action 1:Id 2:Url 3:Engine 4:Config 5:Container
//   6:VideoCodec 7:AudioCodec 8:Protocol 9:AudioChannels
//   10:BytesEstimate 11:ExpiresAt 12:ResolvedHeight
//
// The watchdog transcodes this DTO to JSON via MeshJsonContext's
// ResolveResponse before publishing to the pending TCS — wrapper sees
// identical JSON to the v3.0 path. Reason and Message (on the JSON-side
// ResolveResponse) are NOT on this DTO because server emits them only
// on fallback_native via MsgpackFallbackNativeFrame.
[MessagePackObject(AllowPrivate = true)]
internal sealed partial class MsgpackResolvedFrame
{
    [Key(0)] public string? Action { get; set; }
    [Key(1)] public string? Id { get; set; }
    [Key(2)] public string? Url { get; set; }
    [Key(3)] public string? Engine { get; set; }
    [Key(4)] public string? Config { get; set; }
    [Key(5)] public string? Container { get; set; }
    [Key(6)] public string? VideoCodec { get; set; }
    [Key(7)] public string? AudioCodec { get; set; }
    [Key(8)] public string? Protocol { get; set; }
    [Key(9)] public int? AudioChannels { get; set; }
    [Key(10)] public long? BytesEstimate { get; set; }
    [Key(11)] public string? ExpiresAt { get; set; }
    [Key(12)] public int? ResolvedHeight { get; set; }
}

// "fallback_native" frame. Field order:
//   0:Action 1:Id 2:Reason 3:PublicMessage
[MessagePackObject(AllowPrivate = true)]
internal sealed partial class MsgpackFallbackNativeFrame
{
    [Key(0)] public string? Action { get; set; }
    [Key(1)] public string? Id { get; set; }
    [Key(2)] public string? Reason { get; set; }
    [Key(3)] public string? PublicMessage { get; set; }
}

// "resolve_log" frame (server-side narration, file-only on the client).
// Field order:
//   0:Action 1:Id 2:Message
//
// Note: server's existing JSON resolve_log frame carries a "level"
// field too, but the msgpack wire tag list (Q5) doesn't include it.
// Client's resolve_log dispatch has always been file-only verbose; the
// missing level just means we can't level-filter on the watchdog log.
[MessagePackObject(AllowPrivate = true)]
internal sealed partial class MsgpackResolveLogFrame
{
    [Key(0)] public string? Action { get; set; }
    [Key(1)] public string? Id { get; set; }
    [Key(2)] public string? Message { get; set; }
}
