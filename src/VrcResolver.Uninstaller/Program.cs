using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver.Uninstaller;

internal enum WatchdogMatch
{
    Ours,
    OtherInstall,
    Uninspectable,
}

[SupportedOSPlatform("windows")]
internal static class Program
{
    private static readonly string[] WatchdogExeNames = { "vrcresolver.exe", "WKVRCProxy.exe" };

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

        int closeErrors = RunStep("close-watchdog", () => CloseRunningWatchdog(installDir));
        if (closeErrors > 0)
        {
            Console.WriteLine("Uninstall stopped at close-watchdog -- see the message above.");
            return 2;
        }
        errors += RunStep("restore-yt-dlp", () => RestoreYtDlp(TryFindVrcTools()));
        errors += RunStep("remove-hosts", () => RemoveHostsEntry(watchdogExe));
        errors += RunStep("remove-relay-tls", () => RemoveRelayTls(watchdogExe));
        errors += RunStep("restore-wininet-limit", () => WinInetConnectionLimit.Restore(Console.WriteLine));
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

    internal static WatchdogMatch MatchWatchdog(string? mainModulePath, string installDir)
    {
        if (string.IsNullOrEmpty(mainModulePath)) return WatchdogMatch.Uninspectable;
        string actual = Path.GetFullPath(mainModulePath);
        foreach (string name in WatchdogExeNames)
        {
            string own = Path.GetFullPath(Path.Combine(installDir, name));
            if (string.Equals(actual, own, StringComparison.OrdinalIgnoreCase)) return WatchdogMatch.Ours;
        }
        return WatchdogMatch.OtherInstall;
    }

    private static void CloseRunningWatchdog(string installDir)
    {
        var procs = Process.GetProcessesByName("vrcresolver")
            .Concat(Process.GetProcessesByName("WKVRCProxy"));
        int closed = CloseWatchdogs(procs, installDir, ReadMainModulePath, StopWatchdog);
        if (closed > 0) Thread.Sleep(500);
    }

    private static void StopWatchdog(Process p)
    {
        if (!p.CloseMainWindow()) p.Kill();
        p.WaitForExit(5000);
    }

    internal static string? ReadMainModulePath(Process p) => p.MainModule?.FileName;

    private static bool HasExited(Process p)
    {
        try { return p.HasExited; }
        catch { return false; }
    }

    internal static int CloseWatchdogs(
        IEnumerable<Process> procs, string installDir, Func<Process, string?> readModulePath, Action<Process> stop)
    {
        int closed = 0, skipped = 0, uninspectable = 0, exited = 0;
        foreach (var p in procs)
        {
            using (p)
            {
                string? procExe = null;
                try { procExe = readModulePath(p); }
                catch { }
                if (string.IsNullOrEmpty(procExe) && HasExited(p))
                {
                    exited++;
                    continue;
                }
                switch (MatchWatchdog(procExe, installDir))
                {
                    case WatchdogMatch.Uninspectable:
                        uninspectable++;
                        continue;
                    case WatchdogMatch.OtherInstall:
                        skipped++;
                        continue;
                }
                try
                {
                    stop(p);
                    closed++;
                }
                catch { }
            }
        }
        Console.WriteLine("[uninstall] close-watchdog matched=" + closed + " skipped_other_installs=" + skipped
            + " already_exited=" + exited + " uninspectable=" + uninspectable);
        if (uninspectable > 0)
        {
            throw new InvalidOperationException(
                "a VRCResolver watchdog is running that the uninstaller can't inspect -- close it and run the uninstaller again");
        }
        return closed;
    }

    internal static void RestoreYtDlp(string? toolsDir)
    {
        if (string.IsNullOrEmpty(toolsDir))
        {
            Console.WriteLine("[uninstall] restore-yt-dlp skipped: VRChat Tools dir not found");
            return;
        }

        try { ToolsDirSweeper.Sweep(toolsDir); } catch { }

        try
        {
            RestoreOutcome outcome = ToolsRestore.Restore(toolsDir, out string detail);
            switch (outcome)
            {
                case RestoreOutcome.Failed:
                    Console.Error.WriteLine(
                        "[uninstall] WARNING: yt-dlp.exe was not restored (" + detail + ") -- "
                        + "check Tools/yt-dlp.exe by hand; deleting it makes VRChat re-download its own on next launch.");
                    throw new InvalidOperationException("Tools/yt-dlp.exe could not be restored -- " + detail);
                case RestoreOutcome.RemovedOrphanWrapper:
                    Console.WriteLine(
                        "[uninstall] yt-dlp-og.exe was missing; removed our Tools/yt-dlp.exe so VRChat redownloads its yt-dlp on next session.");
                    break;
                default:
                    Console.WriteLine("[uninstall] restore-yt-dlp " + outcome + ": " + detail);
                    break;
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
