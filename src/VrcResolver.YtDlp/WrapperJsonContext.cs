using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

[JsonSerializable(typeof(ResolveRequest))]
[JsonSerializable(typeof(ResolveResponse))]
[JsonSerializable(typeof(WrapperEventNotify))]
internal sealed partial class WrapperJsonContext : JsonSerializerContext
{
}
