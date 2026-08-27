using System.Text.RegularExpressions;

namespace VrcResolver.Shared;

public static partial class ToolsDirSweeper
{
    [GeneratedRegex(@"^yt-dlp(-og)?\.exe\.(new|stale)-", RegexOptions.IgnoreCase)]
    private static partial Regex SidecarPattern();

    private static readonly string[] LiteralResidueNames =
    {
        "yt-dlp-wrapper.log",
    };

    public static void Sweep(string? toolsDir)
    {
        if (string.IsNullOrEmpty(toolsDir)) return;
        if (!Directory.Exists(toolsDir)) return;
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
