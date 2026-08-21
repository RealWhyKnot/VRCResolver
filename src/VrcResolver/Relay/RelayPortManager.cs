using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

// Picks an ephemeral high port on 127.0.0.1 for the local-relay HTTP
// listener and writes it to %LOCALAPPDATA%Low\vrcresolver\relay_port.txt
// plus relay_scheme.txt so the patched yt-dlp wrapper can read it and emit
// the trust-gateway URL (`{scheme}://localhost.youtube.com:{port}/play/<session>/manifest.<ext>?target=<base64>`) to
// VRChat instead of raw first-party playback proxy URLs that AVPro's allowlist rejects.
//
// Port-persistence behavior across watchdog restarts: read the previous
// port from disk, try to claim it; on success, reuse so any AVPro request
// in flight against the previous port survives the restart. On failure
// (port now busy, e.g. another process took it), pick a fresh ephemeral.
//
// AOT-clean: only TcpListener + File I/O. No reflection.
[SupportedOSPlatform("windows")]
internal sealed class RelayPortManager
{
    public int CurrentPort { get; private set; }

    private readonly string _stateRoot;
    private readonly string _portFile;
    private readonly string _lastPortFile;
    private readonly string _schemeFile;

    public RelayPortManager()
        : this(AppPaths.StateRoot())
    {
    }

    internal RelayPortManager(string stateRoot)
    {
        _stateRoot = stateRoot;
        _portFile = Path.Combine(_stateRoot, "relay_port.txt");
        _lastPortFile = Path.Combine(_stateRoot, "relay_last_port.txt");
        _schemeFile = Path.Combine(_stateRoot, "relay_scheme.txt");
    }

    public bool Initialize()
    {
        Directory.CreateDirectory(_stateRoot);

        int? prev = TryReadPreviousPort();
        if (prev.HasValue && TryClaimSpecificPort(prev.Value))
        {
            CurrentPort = prev.Value;
            WritePortFile(CurrentPort);
            WriteLastPortFile(CurrentPort);
            ConsoleUx.Write(LogComponent.Relay, "reserved local video port " + CurrentPort + " (reused)");
            return true;
        }

        if (!TryClaimEphemeralPort(out int fresh))
        {
            ConsoleUx.Error(LogComponent.Relay, "could not reserve a local video port");
            return false;
        }

        CurrentPort = fresh;
        WritePortFile(CurrentPort);
        WriteLastPortFile(CurrentPort);
        ConsoleUx.Write(LogComponent.Relay, "reserved local video port " + CurrentPort
            + (prev.HasValue ? " (previous port was busy)" : ""));
        return true;
    }

    public bool TryReserveFreshPort(string reason)
    {
        if (!TryClaimEphemeralPort(out int fresh))
        {
            ConsoleUx.Error(LogComponent.Relay, "could not reserve a replacement local video port");
            return false;
        }

        int previous = CurrentPort;
        CurrentPort = fresh;
        WritePortFile(CurrentPort);
        WriteLastPortFile(CurrentPort);

        string detail = previous > 0
            ? " (previous port " + previous.ToString(System.Globalization.CultureInfo.InvariantCulture)
                + " failed"
                + (string.IsNullOrWhiteSpace(reason) ? "" : ": " + reason)
                + ")"
            : "";
        ConsoleUx.Write(LogComponent.Relay, "reserved local video port " + CurrentPort + detail);
        return true;
    }

    public void WriteSchemeFile(string scheme)
    {
        if (!TrustGatewayUrlBuilder.IsAllowedGatewayScheme(scheme))
            scheme = "http";
        TryWriteAtomic(_schemeFile, scheme.ToLowerInvariant());
    }

    public void DeletePortFile()
    {
        try { if (File.Exists(_portFile)) File.Delete(_portFile); }
        catch { /* best-effort cleanup */ }
        try { if (File.Exists(_schemeFile)) File.Delete(_schemeFile); }
        catch { /* best-effort cleanup */ }
    }

    private int? TryReadPreviousPort()
    {
        try
        {
            if (!File.Exists(_lastPortFile)) return null;
            string text = File.ReadAllText(_lastPortFile).Trim();
            if (int.TryParse(text, out int p) && p > 1024 && p < 65536)
                return p;
        }
        catch { /* best-effort */ }
        return null;
    }

    private static bool TryClaimSpecificPort(int port)
    {
        try
        {
            using var listener = new TcpListener(IPAddress.Loopback, port);
            listener.Start();
            listener.Stop();
            return true;
        }
        catch { return false; }
    }

    private static bool TryClaimEphemeralPort(out int port)
    {
        port = 0;
        try
        {
            using var listener = new TcpListener(IPAddress.Loopback, 0);
            listener.Start();
            port = ((IPEndPoint)listener.LocalEndpoint).Port;
            listener.Stop();
            return port > 0;
        }
        catch { return false; }
    }

    private void WritePortFile(int port)
    {
        TryWriteAtomic(_portFile, port.ToString(System.Globalization.CultureInfo.InvariantCulture));
    }

    private void WriteLastPortFile(int port)
    {
        TryWriteAtomic(_lastPortFile, port.ToString(System.Globalization.CultureInfo.InvariantCulture));
    }

    private static void TryWriteAtomic(string path, string content)
    {
        try
        {
            string tmp = path + ".tmp";
            File.WriteAllText(tmp, content);
            File.Move(tmp, path, overwrite: true);
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Relay, "could not write " + Path.GetFileName(path) + ": " + ex.Message);
        }
    }
}
