using System.Diagnostics;
using System.Runtime.Versioning;
using System.Text.RegularExpressions;

namespace VrcResolver.Shared;

public static partial class ToolsDirSweeper
{
    [GeneratedRegex(@"^yt-dlp(-og)?\.exe\.(new|stale)-", RegexOptions.IgnoreCase)]
    private static partial Regex SidecarPattern();

    [GeneratedRegex(@"^_MEI(\d{1,9})$", RegexOptions.IgnoreCase)]
    private static partial Regex PyInstallerTempPattern();

    private static readonly string[] LiteralResidueNames =
    {
        "yt-dlp-wrapper.log",
    };

    public static void Sweep(string? toolsDir)
    {
        if (string.IsNullOrEmpty(toolsDir)) return;
        if (!Directory.Exists(toolsDir)) return;

        if (OperatingSystem.IsWindows()) RestoreBundledStale(toolsDir);

        try
        {
            foreach (string path in Directory.EnumerateFiles(toolsDir))
            {
                string name = Path.GetFileName(path);
                bool match = SidecarPattern().IsMatch(name);
                if (!match)
                {
                    foreach (var literal in LiteralResidueNames)
                    {
                        if (string.Equals(name, literal, StringComparison.OrdinalIgnoreCase))
                        {
                            match = true;
                            break;
                        }
                    }
                }
                if (!match) continue;
                try { File.Delete(path); }
                catch { }
            }
        }
        catch { }

        try
        {
            foreach (string path in Directory.EnumerateDirectories(toolsDir))
            {
                Match m = PyInstallerTempPattern().Match(Path.GetFileName(path));
                if (!m.Success) continue;
                if (!int.TryParse(m.Groups[1].Value, out int pid)) continue;
                if (IsProcessAlive(pid)) continue;
                try { Directory.Delete(path, recursive: true); }
                catch { }
            }
        }
        catch { }
    }

    [SupportedOSPlatform("windows")]
    private static void RestoreBundledStale(string toolsDir)
    {
        try
        {
            string target = Path.Combine(toolsDir, "yt-dlp.exe");
            if (File.Exists(target) || File.Exists(Path.Combine(toolsDir, "yt-dlp-og.exe"))) return;

            string? newest = null;
            foreach (string path in Directory.EnumerateFiles(toolsDir, "yt-dlp.exe.stale-*"))
            {
                if (newest == null
                    || string.CompareOrdinal(Path.GetFileName(path), Path.GetFileName(newest)) > 0)
                {
                    newest = path;
                }
            }
            if (newest == null) return;
            if (WrapperIdentity.Classify(newest) != WrapperKind.VrcBundledYtDlp) return;

            File.Move(newest, target, overwrite: false);
            Logger.WriteFileOnly("[patch] sweep: put VRChat's yt-dlp.exe back from " + Path.GetFileName(newest));
        }
        catch { }
    }

    private static bool IsProcessAlive(int pid)
    {
        if (pid <= 0) return true;
        try
        {
            using Process p = Process.GetProcessById(pid);
            return !p.HasExited;
        }
        catch (ArgumentException) { return false; }
        catch { return true; }
    }

    private static readonly string[] LegacyInstallToolsFiles =
    {
        "yt-dlp-og-fallback.exe",
        "yt-dlp-og-fallback.version.txt",
    };

    public static void SweepLegacyInstallTools(string? installDir)
    {
        if (string.IsNullOrEmpty(installDir)) return;
        string toolsDir = Path.Combine(installDir, "tools");
        if (!Directory.Exists(toolsDir)) return;
        foreach (string name in LegacyInstallToolsFiles)
        {
            string path = Path.Combine(toolsDir, name);
            if (!File.Exists(path)) continue;
            try { File.Delete(path); }
            catch { }
        }
    }
}
