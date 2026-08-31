using MessagePack;

namespace VrcResolver;

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

[MessagePackObject(AllowPrivate = true)]
internal sealed partial class MsgpackFallbackNativeFrame
{
    [Key(0)] public string? Action { get; set; }
    [Key(1)] public string? Id { get; set; }
    [Key(2)] public string? Reason { get; set; }
    [Key(3)] public string? PublicMessage { get; set; }
    [Key(4)] public int? RetryAfterMs { get; set; }
}

[MessagePackObject(AllowPrivate = true)]
internal sealed partial class MsgpackResolveLogFrame
{
    [Key(0)] public string? Action { get; set; }
    [Key(1)] public string? Id { get; set; }
    [Key(2)] public string? Message { get; set; }
}
