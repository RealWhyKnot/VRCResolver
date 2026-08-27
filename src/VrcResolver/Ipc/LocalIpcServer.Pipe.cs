using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.Principal;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed partial class LocalIpcServer
{

    private const uint PIPE_ACCESS_DUPLEX = 0x00000003;
    private const uint FILE_FLAG_OVERLAPPED = 0x40000000;
    private const uint PIPE_TYPE_BYTE = 0x00000000;
    private const uint PIPE_READMODE_BYTE = 0x00000000;
    private const uint PIPE_WAIT = 0x00000000;
    private const uint PIPE_UNLIMITED_INSTANCES = 255;
    private const uint NMPWAIT_USE_DEFAULT_WAIT = 0;

    [StructLayout(LayoutKind.Sequential)]
    private struct SECURITY_ATTRIBUTES
    {
        public uint nLength;
        public IntPtr lpSecurityDescriptor;
        public int bInheritHandle;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern Microsoft.Win32.SafeHandles.SafePipeHandle CreateNamedPipeW(
        string lpName,
        uint dwOpenMode,
        uint dwPipeMode,
        uint nMaxInstances,
        uint nOutBufferSize,
        uint nInBufferSize,
        uint nDefaultTimeOut,
        IntPtr lpSecurityAttributes);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern int ConvertStringSecurityDescriptorToSecurityDescriptorW(
        string sddl,
        int sddlRevision,
        out IntPtr secDesc,
        IntPtr secDescSize);

    [DllImport("kernel32.dll")]
    private static extern IntPtr LocalFree(IntPtr hMem);

    private const int SDDL_REVISION_1 = 1;
    private const string PipeSddl =
        "O:{0}G:{0}D:(A;;0x1f019f;;;{0})(A;;0x1f019f;;;SY)S:(ML;;NW;;;LW)";

    private NamedPipeServerStream CreatePipeWithLowIntegrityLabel(string pipeName)
    {
        string ownerSid = WindowsIdentity.GetCurrent().User?.Value
            ?? throw new InvalidOperationException("could not resolve current user SID");
        string sddl = string.Format(PipeSddl, ownerSid);

        IntPtr secDesc = IntPtr.Zero;
        if (ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, out secDesc, IntPtr.Zero) == 0)
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),
                "ConvertStringSecurityDescriptorToSecurityDescriptor failed");

        try
        {
            var sa = new SECURITY_ATTRIBUTES
            {
                nLength = (uint)Marshal.SizeOf<SECURITY_ATTRIBUTES>(),
                lpSecurityDescriptor = secDesc,
                bInheritHandle = 0,
            };
            IntPtr saPtr = Marshal.AllocHGlobal((int)sa.nLength);
            try
            {
                Marshal.StructureToPtr(sa, saPtr, false);
                string fullName = @"\\.\pipe\" + pipeName;
                var handle = CreateNamedPipeW(
                    fullName,
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    nOutBufferSize: 0,
                    nInBufferSize: 0,
                    nDefaultTimeOut: NMPWAIT_USE_DEFAULT_WAIT,
                    lpSecurityAttributes: saPtr);
                if (handle.IsInvalid)
                    throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),
                        "CreateNamedPipeW failed");
                return new NamedPipeServerStream(PipeDirection.InOut, isAsync: true, isConnected: false, handle);
            }
            finally
            {
                Marshal.FreeHGlobal(saPtr);
            }
        }
        finally
        {
            LocalFree(secDesc);
        }
    }
}
