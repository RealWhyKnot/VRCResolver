using System.Runtime.InteropServices;
using System.Runtime.Versioning;

namespace VrcResolver;

internal enum AudioOutputState { Ok, InUse, NoDevice, Unknown }

internal readonly record struct AudioOutputCheck(AudioOutputState State, string? DeviceName)
{
    public bool BlocksPlayback => State is AudioOutputState.InUse or AudioOutputState.NoDevice;
}

[SupportedOSPlatform("windows")]
internal static class AudioOutputProbe
{
    private const int MmsyserrBadDeviceId = 2;
    private const int MmsyserrAllocated = 4;
    private const int MmsyserrNoDriver = 6;
    private const uint DrvmMapperPreferredGet = 0x2015;
    private const uint NoPreferredDevice = 0xFFFFFFFF;
    private const int WaveOutCapsSize = 84;
    private const int WaveOutCapsNameOffset = 8;

    [StructLayout(LayoutKind.Sequential, Pack = 1)]
    private struct WaveFormatEx
    {
        public ushort FormatTag;
        public ushort Channels;
        public uint SamplesPerSec;
        public uint AvgBytesPerSec;
        public ushort BlockAlign;
        public ushort BitsPerSample;
        public ushort ExtraSize;
    }

    [DllImport("winmm.dll", ExactSpelling = true)]
    private static extern uint waveOutGetNumDevs();

    [DllImport("winmm.dll", ExactSpelling = true)]
    private static extern int waveOutMessage(IntPtr hwo, uint msg, out uint deviceId, out uint status);

    [DllImport("winmm.dll", ExactSpelling = true)]
    private static extern int waveOutGetDevCapsW(nuint deviceId, IntPtr caps, uint size);

    [DllImport("winmm.dll", ExactSpelling = true)]
    private static extern int waveOutOpen(out IntPtr hwo, uint deviceId, in WaveFormatEx format,
        IntPtr callback, IntPtr instance, uint flags);

    [DllImport("winmm.dll", ExactSpelling = true)]
    private static extern int waveOutClose(IntPtr hwo);

    public static AudioOutputCheck CheckDefault()
    {
        try
        {
            if (waveOutGetNumDevs() == 0)
                return new AudioOutputCheck(AudioOutputState.NoDevice, null);

            if (waveOutMessage(new IntPtr(-1), DrvmMapperPreferredGet, out uint deviceId, out _) != 0)
                return new AudioOutputCheck(AudioOutputState.Unknown, null);
            if (deviceId == NoPreferredDevice)
                return new AudioOutputCheck(AudioOutputState.NoDevice, null);

            var format = new WaveFormatEx
            {
                FormatTag = 1,
                Channels = 2,
                SamplesPerSec = 48000,
                AvgBytesPerSec = 192000,
                BlockAlign = 4,
                BitsPerSample = 16,
            };
            int open = waveOutOpen(out IntPtr handle, deviceId, in format, IntPtr.Zero, IntPtr.Zero, 0);
            if (open == 0) waveOutClose(handle);
            return new AudioOutputCheck(Classify(open), DeviceName(deviceId));
        }
        catch (Exception ex) when (ex is DllNotFoundException or EntryPointNotFoundException)
        {
            return new AudioOutputCheck(AudioOutputState.Unknown, null);
        }
    }

    internal static AudioOutputState Classify(int openResult) => openResult switch
    {
        0 => AudioOutputState.Ok,
        MmsyserrAllocated => AudioOutputState.InUse,
        MmsyserrBadDeviceId or MmsyserrNoDriver => AudioOutputState.NoDevice,
        _ => AudioOutputState.Unknown,
    };

    private static string? DeviceName(uint deviceId)
    {
        IntPtr caps = Marshal.AllocHGlobal(WaveOutCapsSize);
        try
        {
            if (waveOutGetDevCapsW(deviceId, caps, WaveOutCapsSize) != 0) return null;
            string name = Marshal.PtrToStringUni(caps + WaveOutCapsNameOffset) ?? "";
            return name.Length == 0 ? null : name;
        }
        finally
        {
            Marshal.FreeHGlobal(caps);
        }
    }
}
