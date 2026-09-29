using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Text;

namespace VrcResolver.Tests;

[SupportedOSPlatform("windows")]
internal sealed class WinInetClient : IDisposable
{
    private const int OpenTypeDirect = 1;
    private const int ServiceHttp = 3;
    private const int OptionMaxConnsPerServer = 73;
    private const int OptionMaxConnsPer10Server = 74;
    private const int QueryStatusCode = 19;
    private const int QueryFlagNumber = 0x20000000;
    private const uint RequestFlags = 0x80000000 | 0x04000000 | 0x00400000 | 0x00080000 | 0x00000200 | 0x00000100;
    private const uint SecureFlags = 0x00800000;

    private readonly IntPtr _session;

    public WinInetClient()
    {
        _session = InternetOpenW("NSPlayer/12.00.26100.1 WMFSDK/12.00.26100.1", OpenTypeDirect, null, null, 0);
        if (_session == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
    }

    public static void SetMaxConnectionsPerServer(int limit)
    {
        int value = limit;
        if (!InternetSetOptionW(IntPtr.Zero, OptionMaxConnsPerServer, ref value, sizeof(int)))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        value = limit;
        if (!InternetSetOptionW(IntPtr.Zero, OptionMaxConnsPer10Server, ref value, sizeof(int)))
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }

    public static int GetMaxConnectionsPerServer()
    {
        int value = 0;
        int len = sizeof(int);
        if (!InternetQueryOptionW(IntPtr.Zero, OptionMaxConnsPerServer, ref value, ref len))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return value;
    }

    public Request Send(string url, string verb = "GET", string? headers = null)
    {
        var uri = new Uri(url);
        IntPtr connect = InternetConnectW(_session, uri.Host, (ushort)uri.Port, null, null, ServiceHttp, 0, IntPtr.Zero);
        if (connect == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        var request = new Request(connect);
        try
        {
            uint flags = RequestFlags | (uri.Scheme == Uri.UriSchemeHttps ? SecureFlags : 0);
            request.Handle = HttpOpenRequestW(connect, verb, uri.PathAndQuery, null, null, IntPtr.Zero, flags, IntPtr.Zero);
            if (request.Handle == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (!HttpSendRequestW(request.Handle, headers, headers?.Length ?? 0, IntPtr.Zero, 0))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            int status = 0;
            int len = sizeof(int);
            if (!HttpQueryInfoW(request.Handle, QueryStatusCode | QueryFlagNumber, ref status, ref len, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            request.Status = status;
            return request;
        }
        catch
        {
            request.Dispose();
            throw;
        }
    }

    public void Dispose() => InternetCloseHandle(_session);

    internal sealed class Request : IDisposable
    {
        private readonly IntPtr _connect;
        private int _closed;

        public Request(IntPtr connect) => _connect = connect;

        public IntPtr Handle { get; set; }
        public int Status { get; set; }

        public int Read(byte[] buffer)
        {
            if (!InternetReadFile(Handle, buffer, buffer.Length, out int read))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return read;
        }

        public long Drain()
        {
            byte[] buffer = new byte[64 * 1024];
            long total = 0;
            int n;
            while ((n = Read(buffer)) > 0) total += n;
            return total;
        }

        public long ReadAtLeast(long bytes)
        {
            byte[] buffer = new byte[64 * 1024];
            long total = 0;
            while (total < bytes)
            {
                int n = Read(buffer);
                if (n == 0) break;
                total += n;
            }
            return total;
        }

        public byte[] ReadAll()
        {
            using var ms = new MemoryStream();
            byte[] buffer = new byte[64 * 1024];
            int n;
            while ((n = Read(buffer)) > 0) ms.Write(buffer, 0, n);
            return ms.ToArray();
        }

        public string ReadText() => Encoding.UTF8.GetString(ReadAll());

        public void Dispose()
        {
            if (Interlocked.Exchange(ref _closed, 1) == 1) return;
            if (Handle != IntPtr.Zero) InternetCloseHandle(Handle);
            InternetCloseHandle(_connect);
        }
    }

    [DllImport("wininet.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr InternetOpenW(string agent, int accessType, string? proxy, string? bypass, int flags);

    [DllImport("wininet.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr InternetConnectW(IntPtr session, string server, ushort port, string? user, string? password, int service, int flags, IntPtr context);

    [DllImport("wininet.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr HttpOpenRequestW(IntPtr connect, string verb, string objectName, string? version, string? referrer, IntPtr acceptTypes, uint flags, IntPtr context);

    [DllImport("wininet.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool HttpSendRequestW(IntPtr request, string? headers, int headersLength, IntPtr optional, int optionalLength);

    [DllImport("wininet.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool HttpQueryInfoW(IntPtr request, int infoLevel, ref int buffer, ref int bufferLength, IntPtr index);

    [DllImport("wininet.dll", SetLastError = true)]
    private static extern bool InternetReadFile(IntPtr file, byte[] buffer, int toRead, out int read);

    [DllImport("wininet.dll", SetLastError = true)]
    private static extern bool InternetCloseHandle(IntPtr handle);

    [DllImport("wininet.dll", SetLastError = true)]
    private static extern bool InternetSetOptionW(IntPtr handle, int option, ref int value, int length);

    [DllImport("wininet.dll", SetLastError = true)]
    private static extern bool InternetQueryOptionW(IntPtr handle, int option, ref int value, ref int length);
}
