using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class ParentProcess
{
    private const int ProcessBasicInformation = 0;
    private static readonly IntPtr CurrentProcess = new(-1);

    [StructLayout(LayoutKind.Sequential)]
    private struct BasicInformation
    {
        public IntPtr ExitStatus;
        public IntPtr PebBaseAddress;
        public IntPtr AffinityMask;
        public IntPtr BasePriority;
        public IntPtr UniqueProcessId;
        public IntPtr InheritedFromUniqueProcessId;
    }

    [DllImport("ntdll.dll", ExactSpelling = true)]
    private static extern int NtQueryInformationProcess(
        IntPtr processHandle,
        int processInformationClass,
        out BasicInformation processInformation,
        int processInformationLength,
        out int returnLength);

    internal static string? Describe()
    {
        try
        {
            int status = NtQueryInformationProcess(
                CurrentProcess,
                ProcessBasicInformation,
                out BasicInformation info,
                Marshal.SizeOf<BasicInformation>(),
                out _);
            if (status != 0) return null;

            int parentId = checked((int)info.InheritedFromUniqueProcessId);
            using Process parent = Process.GetProcessById(parentId);
            return parent.ProcessName + " pid=" + parentId;
        }
        catch
        {
            return null;
        }
    }
}
