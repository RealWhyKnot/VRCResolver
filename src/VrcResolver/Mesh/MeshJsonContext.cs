using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

[JsonSerializable(typeof(WelcomeFrame))]
[JsonSerializable(typeof(ClientHelloFrame))]
[JsonSerializable(typeof(WelcomeCachedFrame))]
[JsonSerializable(typeof(WelcomeCacheFile))]
[JsonSerializable(typeof(WelcomeCacheEntry))]
[JsonSerializable(typeof(ResolveResponse))]
[JsonSerializable(typeof(ResolveRequest))]
[JsonSerializable(typeof(PlaybackFeedbackFrame))]
[JsonSerializable(typeof(CodecInstaller.CodecState))]
[JsonSerializable(typeof(AppSettings))]
[JsonSerializable(typeof(TerminalAppSettings))]
[JsonSerializable(typeof(RelayAppSettings))]
[JsonSerializable(typeof(MaintenanceAppSettings))]
[JsonSerializable(typeof(PlaybackAppSettings))]
[JsonSerializable(typeof(ReportingService.ReportPayload))]
[JsonSerializable(typeof(ResolveCacheFile))]
[JsonSerializable(typeof(ResolveCacheEntry))]
[JsonSerializable(typeof(WrapperEventNotify))]
[JsonSerializable(typeof(TerminalSessionEvent))]
internal sealed partial class MeshJsonContext : JsonSerializerContext
{
}

[JsonSourceGenerationOptions(DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull)]
[JsonSerializable(typeof(ResolveResponse))]
internal sealed partial class MeshFallbackJsonContext : JsonSerializerContext
{
}
