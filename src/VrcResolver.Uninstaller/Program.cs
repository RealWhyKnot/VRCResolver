using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver.Uninstaller;

[SupportedOSPlatform("windows")]
internal static class Program
{
    private static int Main(string[] args)
    {
        try { Console.OutputEncoding = System.Text.Encoding.UTF8; } catch { }
        AppPaths.MigrateFromLegacyProduct(Console.WriteLine);
        Logger.Install("uninstaller");
        CrashHandler.Install("uninstaller");
        int errors = 0;
        string installDir = AppContext.BaseDirectory.TrimEnd('\\', '/');
        string watchdogExe = Path.Combine(installDir, "vrcresolver.exe");

        Console.WriteLine("[uninstall] start installDir=" + installDir);

        errors += RunStep("close-watchdog", () => CloseRunningWatchdog(installDir));
        errors += RunStep("restore-yt-dlp", RestoreYtDlp);
        errors += RunStep("remove-hosts", () => RemoveHostsEntry(watchdogExe));
        errors += RunStep("remove-relay-tls", () => RemoveRelayTls(watchdogExe));
        errors += RunStep("wipe-state", WipeState);
        errors += RunStep("schedule-self-delete", () => ScheduleInstallDirDelete(installDir));

        Console.WriteLine(errors == 0
            ? "VRCResolver uninstalled. The install folder will disappear in a moment."
            : $"Uninstall finished with {errors} non-fatal error(s) — see messages above.");
        return errors == 0 ? 0 : 2;
    }

    private static int RunStep(string step, Action body)
    {
        Console.WriteLine("[uninstall] " + step + " start");
        try
        {
            body();
            Console.WriteLine("[uninstall] " + step + " ok");
            return 0;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine("[uninstall] " + step + " ERROR " + ex.GetType().Name + ": " + ex.Message);
            return 1;
        }
    }

    private static void CloseRunningWatchdog(string installDir)
    {
        string[] ownExes =
        {
            Path.GetFullPath(Path.Combine(installDir, "vrcresolver.exe")),
            Path.GetFullPath(Path.Combine(installDir, "WKVRCProxy.exe")),
        };
        int closed = 0, skipped = 0;
        var procs = Process.GetProcessesByName("vrcresolver")
            .Concat(Process.GetProcessesByName("WKVRCProxy"));
        foreach (var p in procs)
        {
            using (p)
            {
                string? procExe = null;
                try { procExe = p.MainModule?.FileName; }
                catch { skipped++; continue; }
                if (procExe == null
                    || !ownExes.Any(own => string.Equals(Path.GetFullPath(procExe), own,
                        StringComparison.OrdinalIgnoreCase)))
                {
                    skipped++;
                    continue;
                }
                try
                {
                    if (!p.CloseMainWindow()) p.Kill();
                    p.WaitForExit(5000);
                    closed++;
                }
                catch { }
            }
        }
        Console.WriteLine("[uninstall] close-watchdog matched=" + closed + " skipped_other_installs=" + skipped);
        if (closed > 0) Thread.Sleep(500);
    }

    private static void RestoreYtDlp()
    {
        string? toolsDir = TryFindVrcTools();
        if (string.IsNullOrEmpty(toolsDir))
        {
            Console.WriteLine("[uninstall] restore-yt-dlp skipped: VRChat Tools dir not found");
            return;
        }

        try { ToolsDirSweeper.Sweep(toolsDir); } catch { }

        string target = Path.Combine(toolsDir, "yt-dlp.exe");
        string backup = Path.Combine(toolsDir, "yt-dlp-og.exe");

        try
        {
            if (File.Exists(backup))
            {
                try
                {
                    File.Move(backup, target, overwrite: true);
                    return;
                }
                catch (IOException)
                {
                }

                try
                {
                    if (File.Exists(target))
                    {
                        string stale = target + ".stale-" + DateTime.UtcNow.ToString("yyyyMMddHHmmssfff");
                        File.Move(target, stale);
                    }
                    File.Move(backup, target);
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine("yt-dlp.exe restore failed: " + ex.Message);
                    throw;
                }
                return;
            }
            if (File.Exists(target))
            {
                try
                {
                    File.Delete(target);
                    Console.WriteLine(
                        "[uninstall] yt-dlp-og.exe was missing; deleted Tools/yt-dlp.exe so VRChat redownloads its yt-dlp on next session.");
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine(
                        "[uninstall] WARNING: could not delete Tools/yt-dlp.exe: " + ex.Message + " -- "
                        + "delete it manually so VRChat re-downloads on next launch.");
                    throw new InvalidOperationException(
                        "Tools/yt-dlp.exe could not be removed -- backup missing and delete failed");
                }
            }
        }
        finally
        {
            try { ToolsDirSweeper.Sweep(toolsDir); } catch { }
        }
    }

    private static void RemoveHostsEntry(string watchdogExe)
    {
        if (!File.Exists(watchdogExe))
        {
            Console.WriteLine("[uninstall] remove-hosts skipped: watchdog exe missing (can't re-exec elevated)");
            return;
        }
        if (!HostsFileContainsBypassEntry())
        {
            Console.WriteLine("[uninstall] remove-hosts skipped: entry already absent");
            return;
        }
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = watchdogExe,
                Arguments = "--remove-hosts-entry",
                UseShellExecute = true,
                Verb = "runas",
                WindowStyle = ProcessWindowStyle.Hidden
            };
            using var proc = Process.Start(psi);
            proc?.WaitForExit(10000);
        }
        catch (System.ComponentModel.Win32Exception)
        {
            Console.WriteLine("UAC declined — hosts entry left in place. Remove it manually if desired.");
        }
    }

    private static void RemoveRelayTls(string watchdogExe)
    {
        if (!File.Exists(watchdogExe))
        {
            Console.WriteLine("[uninstall] remove-relay-tls skipped: watchdog exe missing (can't re-exec elevated)");
            return;
        }

        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = watchdogExe,
                Arguments = "--local-relay-tls-remove",
                UseShellExecute = true,
                Verb = "runas",
                WindowStyle = ProcessWindowStyle.Hidden
            };
            using var proc = Process.Start(psi);
            proc?.WaitForExit(30000);
            if (proc != null && !proc.HasExited)
                Console.WriteLine("[uninstall] remove-relay-tls elevation child still running after 30s");
        }
        catch (System.ComponentModel.Win32Exception)
        {
            Console.WriteLine("UAC declined -- localhost.youtube.com HTTPS certificate/bindings may be left in place.");
        }
    }

    private const string BypassMarkerHost = "localhost.youtube.com";
    private static bool HostsFileContainsBypassEntry()
    {
        string p = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.System),
            "drivers", "etc", "hosts");
        if (!File.Exists(p)) return false;
        try
        {
            using var fs = new FileStream(p, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            using var sr = new StreamReader(fs);
            string? line;
            while ((line = sr.ReadLine()) != null)
            {
                string t = line.Trim();
                if (t.StartsWith('#')) continue;
                if (t.Contains("127.0.0.1") && t.Contains(BypassMarkerHost)) return true;
            }
        }
        catch { }
        return false;
    }

    private static void WipeState()
    {
        Logger.Close();

        string localApp = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        string programData = Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData);
        string legacyLowRoot = Path.Combine(
            Path.GetDirectoryName(AppPaths.StateRoot()) ?? (localApp + "Low"),
            LegacyCompat.LegacyStateDirName);
        string[] roots =
        {
            AppPaths.StateRoot(),
            legacyLowRoot,
            Path.Combine(localApp, LegacyCompat.LegacyStateDirName),
            AppPaths.ProgramDataRoot(),
            Path.Combine(programData, LegacyCompat.LegacyStateDirName),
        };

        int wiped = 0;
        foreach (string root in roots)
        {
            if (!Directory.Exists(root)) continue;
            try { Directory.Delete(root, recursive: true); wiped++; }
            catch (Exception ex) { Console.Error.WriteLine("[uninstall] could not wipe " + root + ": " + ex.Message); }
        }

        Console.WriteLine("[uninstall] wipe-state directories_wiped=" + wiped);
    }

    private static void ScheduleInstallDirDelete(string installDir)
    {
        if (installDir.Contains('"'))
            throw new InvalidOperationException("install dir contains a quote character: " + installDir);

        string log = Path.Combine(Path.GetTempPath(),
            "vrcresolver-uninstall-rmdir-" + DateTime.UtcNow.ToString("yyyyMMddHHmmss") + ".log");
        string cmd = $"/c (ping 127.0.0.1 -n 4 > nul) & (rmdir /s /q \"{installDir}\") > \"{log}\" 2>&1";
        var psi = new ProcessStartInfo("cmd.exe", cmd)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = Path.GetTempPath(),
        };
        Process.Start(psi);
        Console.WriteLine("[uninstall] schedule-self-delete cmd-log=" + log);
    }

    private static string? TryFindVrcTools()
    {
        string p = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData) + "Low",
            "VRChat", "VRChat", "Tools");
        return Directory.Exists(p) ? p : null;
    }
}
