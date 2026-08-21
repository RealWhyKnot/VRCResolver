using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

// Verifies which extension-backed video codecs this machine can actually decode,
// by asking Media Foundation for a registered decoder (MFTEnumEx). That is the
// truth regardless of how the decoder arrived — store package, device-manufacturer
// variant, or a hardware MFT — where an AppX name check only sees store installs.
//
// The watchdog probes once at startup and again after a confirmed codec install;
// LocalIpcServer reads the snapshot to prune the wrapper's static accept_codecs
// claim down to verified reality before it goes to the server. A failed probe
// yields null and the claim conservatively drops every extension-backed codec.
[SupportedOSPlatform("windows")]
internal static class CodecCapabilityProbe
{
    private static readonly object _lock = new();
    private static HashSet<string>? _verified;

    public static IReadOnlySet<string>? VerifiedVideoCodecs
    {
        get { lock (_lock) return _verified; }
    }

    public static void Refresh()
    {
        HashSet<string>? result = null;
        try
        {
            // MFSTARTUP_LITE; ref-counted and never shut down — the watchdog is
            // long-lived and process exit reclaims it.
            if (MFStartup(MfVersion, MfStartupLite) >= 0)
            {
                var set = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                foreach (var codec in WireConstants.ExtensionBackedVideoCodecs)
                {
                    if (ProbeDecoder(codec) == true) set.Add(codec);
                }
                result = set;
            }
        }
        catch
        {
            result = null;
        }
        lock (_lock) _verified = result;
        Logger.WriteFileOnly("[codec] capability probe: "
            + (result == null ? "unavailable" : result.Count == 0 ? "none" : string.Join(",", result)));
    }

    // Fresh MFTEnumEx for one codec token; null = probe machinery unavailable.
    internal static bool? ProbeDecoder(string codec)
    {
        Guid subtype;
        switch (codec)
        {
            case "h265": subtype = MfVideoFormatHevc; break;
            case "vp9": subtype = MfVideoFormatVp90; break;
            case "av1": subtype = MfVideoFormatAv01; break;
            default: return null;
        }
        try
        {
            var input = new MftRegisterTypeInfo { MajorType = MfMediaTypeVideo, Subtype = subtype };
            int hr = MFTEnumEx(MftCategoryVideoDecoder, MftEnumSyncAsyncHardware,
                in input, IntPtr.Zero, out IntPtr activates, out int count);
            if (hr < 0) return null;
            if (activates != IntPtr.Zero)
            {
                for (int i = 0; i < count; i++)
                {
                    IntPtr p = Marshal.ReadIntPtr(activates, i * IntPtr.Size);
                    if (p != IntPtr.Zero) Marshal.Release(p);
                }
                Marshal.FreeCoTaskMem(activates);
            }
            return count > 0;
        }
        catch
        {
            return null;
        }
    }

    internal static void SetVerifiedForTests(HashSet<string>? verified)
    {
        lock (_lock) _verified = verified;
    }

    private const int MfVersion = 0x0002_0070;
    private const int MfStartupLite = 1;
    private const int MftEnumSyncAsyncHardware = 0x1 | 0x2 | 0x4;

    private static readonly Guid MftCategoryVideoDecoder = new("d6c02d4b-6833-45b4-971a-05a4b04bab91");
    private static readonly Guid MfMediaTypeVideo = new("73646976-0000-0010-8000-00aa00389b71");
    // FourCC subtypes: first GUID field is the little-endian FourCC.
    private static readonly Guid MfVideoFormatHevc = new("43564548-0000-0010-8000-00aa00389b71");
    private static readonly Guid MfVideoFormatVp90 = new("30395056-0000-0010-8000-00aa00389b71");
    private static readonly Guid MfVideoFormatAv01 = new("31305641-0000-0010-8000-00aa00389b71");

    [StructLayout(LayoutKind.Sequential)]
    private struct MftRegisterTypeInfo
    {
        public Guid MajorType;
        public Guid Subtype;
    }

    [DllImport("mfplat.dll", ExactSpelling = true)]
    private static extern int MFStartup(int version, int flags);

    [DllImport("mfplat.dll", ExactSpelling = true)]
    private static extern int MFTEnumEx(Guid guidCategory, int flags,
        in MftRegisterTypeInfo inputType, IntPtr outputType,
        out IntPtr activates, out int count);
}
