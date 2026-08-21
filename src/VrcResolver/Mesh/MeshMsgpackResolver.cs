using MessagePack;

namespace VrcResolver;

// Source-generated MessagePack resolver. The MessagePackAnalyzer 3.1.4
// package's source generator emits a partial class implementation that
// produces a static formatter for every [MessagePackObject] type
// reachable from this assembly (MsgpackResolvedFrame /
// MsgpackFallbackNativeFrame / MsgpackResolveLogFrame in
// MeshMsgpackDtos.cs).
//
// Used by MeshClient.DispatchBinaryFrameAsync via a CompositeResolver
// of (MeshMsgpackResolver.Instance, BuiltinResolver.Instance). The
// composite shape deliberately omits StandardResolver / Standard's
// dynamic fallbacks: those reach the DynamicObjectResolver +
// DynamicGenericResolver code paths inside MessagePack.dll which use
// Reflection.Emit at runtime to build IL formatters on the fly. Under
// AOT publish Reflection.Emit is unsupported (throws
// PlatformNotSupportedException), so the dynamic chain MUST never be
// routed to.
//
// Probe-validated: see project_v3_1_msgpack_client.md decode-CPU
// benchmark + AOT publish smoke. The composite of generated + builtin
// only successfully decodes every hot-path frame the server emits;
// runtime never executes a dynamic-resolver code path even though the
// trimmer keeps the dynamic types in the binary (the IL3050 warning
// cluster is suppressed at the csproj level for this reason).
[GeneratedMessagePackResolver]
internal partial class MeshMsgpackResolver
{
}
