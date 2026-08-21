using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

// Source-generated JSON metadata for every DTO the watchdog
// serializes or deserializes. Used in place of reflection-based
// JsonSerializer.Serialize<T> / Deserialize<T> so the watchdog can
// publish under PublishAot=true with TrimMode=full -- without this
// context the trimmer + AOT compiler can't see which property
// accessors are reachable, the JSON code path falls back to runtime
// reflection, and AOT publish bails with IL2026/IL3050 (or worse,
// silently produces a binary that throws PlatformNotSupportedException
// at first deserialize).
//
// Scope after the AOT migration (commit 2/3 of project_watchdog_aot_migration.md):
// every JsonSerializer.* site in src/VrcResolver is routed through here
// or through MeshFallbackJsonContext below. The Updater / Uninstaller
// have their own paths (UpdaterJsonContext / no JSON in Uninstaller).
[JsonSerializable(typeof(WelcomeFrame))]
[JsonSerializable(typeof(ClientHelloFrame))]
[JsonSerializable(typeof(WelcomeCachedFrame))]
[JsonSerializable(typeof(WelcomeCacheFile))]
[JsonSerializable(typeof(WelcomeCacheEntry))]
// v3.1: ResolveResponse is the JSON shape we transcode TO when the
// server sent a msgpack-Binary frame on the hot path. Wrapper consumes
// JSON-on-pipe regardless of WS-side wire format; this serializer
// context produces the bytes the existing MeshResolveResult.Frame
// passthrough writes to LocalIpcServer's pipe write.
[JsonSerializable(typeof(ResolveResponse))]
// AOT migration (commit 2 of 3): ResolveRequest goes outbound on the
// pipe (LocalIpcServer.HandleAsync deserialize) and on the WS
// (MeshClient.ResolveAsync serialize). Both paths must use source-gen
// for AOT-clean publish; reflection-based JsonSerializer.Deserialize<T>
// throws PlatformNotSupportedException under AOT.
[JsonSerializable(typeof(ResolveRequest))]
// MeshClient.BuildPlaybackFeedbackPayload (R2 watchdog -> server frame).
// Pre-AOT this serialized a Dictionary<string, object?> via reflection;
// migrated here to a typed DTO so the source-gen path covers it.
[JsonSerializable(typeof(PlaybackFeedbackFrame))]
// CodecInstaller persistence (state file at LocalLow root).
// Watchdog-internal state file, JSON-on-disk.
[JsonSerializable(typeof(CodecInstaller.CodecState))]
// User-editable watchdog settings at LocalLow\vrcresolver\settings.json.
// Kept in this source-gen context so settings remain AOT-safe.
[JsonSerializable(typeof(AppSettings))]
[JsonSerializable(typeof(TerminalAppSettings))]
[JsonSerializable(typeof(RelayAppSettings))]
[JsonSerializable(typeof(MaintenanceAppSettings))]
// ReportingService outbound /report frames (anonymous failure
// telemetry; gated by VRCRESOLVER_ANONYMOUS_REPORTING).
[JsonSerializable(typeof(ReportingService.ReportPayload))]
// v3.2: ResolveCache persistent state at LocalLow root.
// resolve_cache.json -- per-(url, player, format, node) fingerprint
// keyed dict of cached `resolved` ResolveResponse + server-issued
// expires_at timestamp. ResolveResponse is already covered above; the
// file + entry types live alongside.
[JsonSerializable(typeof(ResolveCacheFile))]
[JsonSerializable(typeof(ResolveCacheEntry))]
// v3.2: wrapper -> watchdog og-fallback notification. Wrapper opens a
// fresh pipe, sends one of these, closes. Watchdog dispatches in
// LocalIpcServer.HandleAsync and emits a single console line surfacing
// the fallback fact to the operator.
[JsonSerializable(typeof(WrapperEventNotify))]
[JsonSerializable(typeof(TerminalSessionEvent))]
internal sealed partial class MeshJsonContext : JsonSerializerContext
{
}

// Companion context for the LocalIpcServer.WriteFallbackAsync path,
// which needs DefaultIgnoreCondition.WhenWritingNull so the wire shape
// stays v1-identical for v1 patched-yt-dlp consumers (ResolveResponse
// has many nullable v2 fields that must be omitted, not serialized as
// "field":null). Source-gen produces a parallel formatter set for
// ResolveResponse with the WhenWritingNull options applied at
// generation time.
[JsonSourceGenerationOptions(DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull)]
[JsonSerializable(typeof(ResolveResponse))]
internal sealed partial class MeshFallbackJsonContext : JsonSerializerContext
{
}
