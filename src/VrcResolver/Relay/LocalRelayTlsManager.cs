using System.ComponentModel;
using System.Diagnostics;
using System.Net;
using System.Runtime.Versioning;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class LocalRelayTlsManager
{
    public const string BootstrapArg = "--local-relay-tls-bootstrap";
    public const string RemoveArg = "--local-relay-tls-remove";

    private const string PortsFileName = "localhost-youtube-relay-ports.txt";
    // Cosmetic store label on newly created certs. Reuse/removal matching
    // is by CN + self-signed check (IsOurCertificate), so certs created
    // under the previous label keep working and get cleaned up the same.
    private const string FriendlyName = "VRCResolver localhost.youtube.com relay";
    private const string HostName = HostsManager.MarkerHost;
    private const string LoopbackIp = "127.0.0.1";
    private const int BootstrapTimeoutMs = 60000;
    private static readonly Guid AppId = new("4d2d4eb4-f953-49c4-ad70-c2c7b4f5de1d");

    public static bool TryEnsureReadyForPort(int port)
    {
        if (!IsValidPort(port)) return false;
        if (TlsDisabled()) return false;

        try
        {
            if (IsReady(port)) return true;
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Relay, "HTTPS readiness check failed: " + ex.Message);
        }

        ConsoleUx.Write(LogComponent.Relay, "configuring HTTPS for localhost.youtube.com -- UAC prompt incoming.");
        if (!ReexecElevated(BootstrapArg + " " + port.ToString(System.Globalization.CultureInfo.InvariantCulture)))
            return false;

        try
        {
            if (IsReady(port))
            {
                ConsoleUx.Write(LogComponent.Relay, "HTTPS ready for localhost.youtube.com:" + port);
                return true;
            }
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Relay, "HTTPS verification failed after elevation: " + ex.Message);
        }

        ConsoleUx.Warn(LogComponent.Relay, "HTTPS setup did not complete; falling back to HTTP relay.");
        return false;
    }

    public static int RunBootstrapInElevatedChild(string[] args)
    {
        if (args.Length < 2 || !int.TryParse(args[1], out int port) || !IsValidPort(port))
        {
            ConsoleUx.Error(LogComponent.Relay, "HTTPS setup failed: invalid port.");
            return 2;
        }

        try
        {
            Directory.CreateDirectory(ProgramDataRoot());
            using var cert = EnsureCertificateInstalled();
            EnsurePortBinding(port, cert.Thumbprint);
            RecordPort(port);
            return 0;
        }
        catch (Exception ex)
        {
            ConsoleUx.Error(LogComponent.Relay, "HTTPS setup failed: " + ex.GetType().Name + ": " + ex.Message);
            return 1;
        }
    }

    public static int RunRemoveInElevatedChild()
    {
        int errors = 0;
        foreach (int port in ReadRecordedPorts())
        {
            try { DeletePortBinding(port); }
            catch (Exception ex)
            {
                errors++;
                ConsoleUx.Warn(LogComponent.Relay, "HTTPS cleanup could not remove sslcert for port " + port + ": " + ex.Message);
            }
        }

        try { RemoveInstalledCertificates(StoreName.My); }
        catch (Exception ex) { errors++; ConsoleUx.Warn(LogComponent.Relay, "HTTPS cleanup could not remove personal certificate: " + ex.Message); }
        try { RemoveInstalledCertificates(StoreName.Root); }
        catch (Exception ex) { errors++; ConsoleUx.Warn(LogComponent.Relay, "HTTPS cleanup could not remove trusted root certificate: " + ex.Message); }
        try { DeleteLegacyPfxIfPresent(); }
        catch (Exception ex) { errors++; ConsoleUx.Warn(LogComponent.Relay, "HTTPS cleanup could not delete legacy certificate file: " + ex.Message); }
        try { if (File.Exists(PortsPath())) File.Delete(PortsPath()); }
        catch (Exception ex) { errors++; ConsoleUx.Warn(LogComponent.Relay, "HTTPS cleanup could not delete ports file: " + ex.Message); }
        try
        {
            string root = ProgramDataRoot();
            if (Directory.Exists(root) && !Directory.EnumerateFileSystemEntries(root).Any())
                Directory.Delete(root);
        }
        catch { /* best-effort */ }
        // This child runs elevated, so it's the right place to clear the
        // pre-rename ProgramData dir (its files were written elevated and
        // may not be deletable from the unelevated uninstaller).
        try
        {
            string legacyRoot = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
                LegacyCompat.LegacyStateDirName);
            if (Directory.Exists(legacyRoot))
                Directory.Delete(legacyRoot, recursive: true);
        }
        catch { /* best-effort */ }

        return errors == 0 ? 0 : 1;
    }

    private static bool IsReady(int port)
    {
        using X509Certificate2? cert = FindInstalledCertificate(StoreName.My);
        return cert != null
            && IsCertificateTrusted(cert.Thumbprint)
            && BindingMatches(port, cert.Thumbprint);
    }

    private static X509Certificate2 EnsureCertificateInstalled()
    {
        X509Certificate2 cert = LoadOrCreateCertificate();
        InstallCertificate(StoreName.My, cert);
        InstallCertificate(StoreName.Root, cert);
        RemoveOldCertificates(StoreName.My, cert.Thumbprint);
        RemoveOldCertificates(StoreName.Root, cert.Thumbprint);
        return cert;
    }

    private static X509Certificate2 LoadOrCreateCertificate()
    {
        X509Certificate2? installed = FindInstalledCertificate(StoreName.My);
        if (installed != null) return installed;

        using RSA rsa = RSA.Create(2048);
        var request = new CertificateRequest(
            "CN=" + HostName,
            rsa,
            HashAlgorithmName.SHA256,
            RSASignaturePadding.Pkcs1);

        var san = new SubjectAlternativeNameBuilder();
        san.AddDnsName(HostName);
        san.AddDnsName("localhost");
        san.AddIpAddress(IPAddress.Loopback);
        request.CertificateExtensions.Add(san.Build());
        request.CertificateExtensions.Add(new X509BasicConstraintsExtension(false, false, 0, false));
        request.CertificateExtensions.Add(new X509KeyUsageExtension(
            X509KeyUsageFlags.DigitalSignature | X509KeyUsageFlags.KeyEncipherment,
            false));
        request.CertificateExtensions.Add(new X509EnhancedKeyUsageExtension(
            new OidCollection { new Oid("1.3.6.1.5.5.7.3.1") },
            false));

        var notBefore = DateTimeOffset.UtcNow.AddDays(-1);
        var notAfter = DateTimeOffset.UtcNow.AddYears(5);
        using X509Certificate2 raw = request.CreateSelfSigned(notBefore, notAfter);
        byte[] pfx = raw.Export(X509ContentType.Pfx, "");

        var cert = X509CertificateLoader.LoadPkcs12(
            pfx,
            "",
            X509KeyStorageFlags.MachineKeySet
            | X509KeyStorageFlags.PersistKeySet,
            Pkcs12LoaderLimits.Defaults);
        TrySetFriendlyName(cert);
        return cert;
    }

    private static bool IsUsableCertificate(X509Certificate2 cert)
    {
        return cert.HasPrivateKey
            && cert.NotAfter.ToUniversalTime() > DateTime.UtcNow.AddDays(30)
            && IsOurCertificate(cert);
    }

    private static void InstallCertificate(StoreName storeName, X509Certificate2 cert)
    {
        using var store = new X509Store(storeName, StoreLocation.LocalMachine);
        store.Open(OpenFlags.ReadWrite);
        bool exists = store.Certificates
            .Find(X509FindType.FindByThumbprint, cert.Thumbprint, validOnly: false)
            .Count > 0;
        if (!exists)
            store.Add(cert);
    }

    private static X509Certificate2? FindInstalledCertificate(StoreName storeName)
    {
        using var store = new X509Store(storeName, StoreLocation.LocalMachine);
        store.Open(OpenFlags.ReadOnly);
        X509Certificate2? best = null;
        foreach (var cert in store.Certificates)
        {
            if (!IsUsableCertificate(cert)) continue;
            if (best == null || cert.NotAfter > best.NotAfter)
                best = new X509Certificate2(cert);
        }
        return best;
    }

    private static bool IsCertificateTrusted(string thumbprint)
    {
        using var store = new X509Store(StoreName.Root, StoreLocation.LocalMachine);
        store.Open(OpenFlags.ReadOnly);
        return store.Certificates
            .Find(X509FindType.FindByThumbprint, thumbprint, validOnly: false)
            .Count > 0;
    }

    private static void RemoveOldCertificates(StoreName storeName, string keepThumbprint)
    {
        using var store = new X509Store(storeName, StoreLocation.LocalMachine);
        store.Open(OpenFlags.ReadWrite);
        var toRemove = new List<X509Certificate2>();
        foreach (var cert in store.Certificates)
        {
            if (!IsOurCertificate(cert)) continue;
            if (string.Equals(cert.Thumbprint, keepThumbprint, StringComparison.OrdinalIgnoreCase)) continue;
            toRemove.Add(cert);
        }
        foreach (var cert in toRemove)
            store.Remove(cert);
    }

    private static void RemoveInstalledCertificates(StoreName storeName)
    {
        using var store = new X509Store(storeName, StoreLocation.LocalMachine);
        store.Open(OpenFlags.ReadWrite);
        var toRemove = new List<X509Certificate2>();
        foreach (var cert in store.Certificates)
        {
            if (IsOurCertificate(cert))
                toRemove.Add(cert);
        }
        foreach (var cert in toRemove)
            store.Remove(cert);
    }

    private static bool IsOurCertificate(X509Certificate2 cert)
    {
        return cert.Subject.Contains("CN=" + HostName, StringComparison.OrdinalIgnoreCase)
            && string.Equals(cert.Subject, cert.Issuer, StringComparison.OrdinalIgnoreCase);
    }

    private static void TrySetFriendlyName(X509Certificate2 cert)
    {
        try { cert.FriendlyName = FriendlyName; }
        catch { /* best-effort */ }
    }

    private static void EnsurePortBinding(int port, string thumbprint)
    {
        if (BindingMatches(port, thumbprint)) return;
        DeletePortBinding(port);
        string args = "http add sslcert ipport=" + LoopbackIp + ":" + port
            + " certhash=" + StripThumbprint(thumbprint)
            + " appid={" + AppId + "}"
            + " certstorename=MY";
        var result = RunNetsh(args);
        if (result.ExitCode != 0)
            throw new InvalidOperationException("netsh add sslcert failed: " + result.StdErr + " " + result.StdOut);
    }

    private static bool BindingMatches(int port, string thumbprint)
    {
        var result = RunNetsh("http show sslcert ipport=" + LoopbackIp + ":" + port);
        if (result.ExitCode != 0) return false;
        string combined = (result.StdOut + "\n" + result.StdErr).Replace(" ", "", StringComparison.Ordinal);
        return combined.Contains(StripThumbprint(thumbprint), StringComparison.OrdinalIgnoreCase)
            && combined.Contains(AppId.ToString("D"), StringComparison.OrdinalIgnoreCase);
    }

    private static void DeletePortBinding(int port)
    {
        _ = RunNetsh("http delete sslcert ipport=" + LoopbackIp + ":" + port);
    }

    private static (int ExitCode, string StdOut, string StdErr) RunNetsh(string arguments)
    {
        using var proc = new Process();
        proc.StartInfo = new ProcessStartInfo("netsh", arguments)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        proc.Start();
        string stdout = proc.StandardOutput.ReadToEnd();
        string stderr = proc.StandardError.ReadToEnd();
        proc.WaitForExit(15000);
        if (!proc.HasExited)
        {
            try { proc.Kill(); } catch { /* best-effort */ }
            return (1, stdout, stderr + " netsh timed out");
        }
        return (proc.ExitCode, stdout, stderr);
    }

    private static bool ReexecElevated(string args)
    {
        string? exe = Process.GetCurrentProcess().MainModule?.FileName;
        if (string.IsNullOrEmpty(exe)) return false;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = exe,
                Arguments = args,
                UseShellExecute = true,
                Verb = "runas",
                WindowStyle = ProcessWindowStyle.Hidden
            };
            using var proc = Process.Start(psi);
            proc?.WaitForExit(BootstrapTimeoutMs);
            if (proc != null && !proc.HasExited)
            {
                ConsoleUx.Warn(LogComponent.Relay, "HTTPS elevation child still running after 60s -- falling back to HTTP.");
                return false;
            }
            return proc == null || proc.ExitCode == 0;
        }
        catch (Win32Exception)
        {
            ConsoleUx.Write(LogComponent.Relay, "UAC declined -- HTTPS relay not configured.");
            return false;
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Relay, "HTTPS elevation error: " + ex.Message);
            return false;
        }
    }

    private static void RecordPort(int port)
    {
        string path = PortsPath();
        Directory.CreateDirectory(ProgramDataRoot());
        var ports = ReadRecordedPorts().ToHashSet();
        ports.Add(port);
        File.WriteAllLines(path, ports.OrderBy(p => p).Select(p => p.ToString(System.Globalization.CultureInfo.InvariantCulture)));
    }

    private static IEnumerable<int> ReadRecordedPorts()
    {
        var ports = new HashSet<int>();
        AddPortFromFile(Path.Combine(AppPaths.StateRoot(), "relay_last_port.txt"), ports);
        AddPortFromFile(Path.Combine(AppPaths.StateRoot(), "relay_port.txt"), ports);
        string path = PortsPath();
        if (File.Exists(path))
        {
            foreach (string line in File.ReadAllLines(path))
            {
                if (int.TryParse(line.Trim(), out int port) && IsValidPort(port))
                    ports.Add(port);
            }
        }
        return ports;
    }

    private static void AddPortFromFile(string path, HashSet<int> ports)
    {
        try
        {
            if (!File.Exists(path)) return;
            if (int.TryParse(File.ReadAllText(path).Trim(), out int port) && IsValidPort(port))
                ports.Add(port);
        }
        catch { /* best-effort */ }
    }

    private static bool IsValidPort(int port) => port > 1024 && port < 65536;

    private static bool TlsDisabled()
    {
        string? value = LegacyCompat.GetEnvWithLegacyFallback("DISABLE_LOCAL_RELAY_HTTPS");
        return string.Equals(value, "1", StringComparison.Ordinal)
            || string.Equals(value, "true", StringComparison.OrdinalIgnoreCase);
    }

    private static string StripThumbprint(string thumbprint)
        => thumbprint.Replace(" ", "", StringComparison.Ordinal).ToUpperInvariant();

    private static string ProgramDataRoot() => AppPaths.ProgramDataRoot();

    private static string PortsPath() => Path.Combine(ProgramDataRoot(), PortsFileName);

    private static void DeleteLegacyPfxIfPresent()
    {
        string legacy = Path.Combine(ProgramDataRoot(), "localhost-youtube-relay.pfx");
        if (File.Exists(legacy)) File.Delete(legacy);
    }
}
