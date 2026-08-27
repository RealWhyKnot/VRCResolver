using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class HostsManager
{
    public const string MarkerHost = "localhost.youtube.com";
    private const string MarkerIp = "127.0.0.1";
    public const string AddArg = "--add-hosts-entry";
    public const string RemoveArg = "--remove-hosts-entry";

    private static string HostsPath =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "drivers", "etc", "hosts");

    public static bool IsBypassActive() => TryReadBypassState(out bool present, out _) && present;

    public static bool TryReadBypassState(out bool present, out string? errorReason)
    {
        present = false;
        errorReason = null;
        if (!File.Exists(HostsPath)) { errorReason = "hosts file missing"; return true; }
        Exception? lastEx = null;
        for (int attempt = 1; attempt <= 3; attempt++)
        {
            try
            {
                using var fs = new FileStream(HostsPath, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
                using var sr = new StreamReader(fs);
                string? line;
                while ((line = sr.ReadLine()) != null)
                {
                    if (LineIsBypassEntry(line)) { present = true; return true; }
                }
                return true;
            }
            catch (IOException ex) { lastEx = ex; }
            catch (UnauthorizedAccessException ex) { lastEx = ex; break; }
            catch (Exception ex) { lastEx = ex; break; }
            if (attempt < 3) Thread.Sleep(200);
        }
        errorReason = lastEx?.GetType().Name + ": " + (lastEx?.Message ?? "<unknown>");
        return false;
    }

    public static bool LineIsBypassEntry(string? rawLine)
    {
        if (string.IsNullOrWhiteSpace(rawLine)) return false;
        string trimmed = rawLine.TrimStart();
        if (trimmed.Length == 0 || trimmed[0] == '#') return false;

        int hashIdx = trimmed.IndexOf('#');
        string body = (hashIdx >= 0 ? trimmed[..hashIdx] : trimmed).Trim();
        if (body.Length == 0) return false;

        var tokens = body.Split(new[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries);
        if (tokens.Length < 2) return false;
        if (!tokens[0].Equals(MarkerIp, StringComparison.Ordinal)) return false;
        for (int i = 1; i < tokens.Length; i++)
        {
            if (tokens[i].Equals(MarkerHost, StringComparison.OrdinalIgnoreCase)) return true;
        }
        return false;
    }

    public static void EnsureBypassEntryOrPrompt()
    {
        if (IsBypassActive()) return;
        ConsoleUx.Write(LogComponent.Hosts, "adding entry for public-instance support -- UAC prompt incoming.");
        if (!ReexecElevated(AddArg)) return;
        if (IsBypassActive())
            ConsoleUx.Write(LogComponent.Hosts, "added " + MarkerIp + " " + MarkerHost);
        else
            ConsoleUx.Warn(LogComponent.Hosts, "entry not present after elevation -- public-instance support may not work.");
    }

    public static void RemoveBypassEntryIfPresent()
    {
        if (!IsBypassActive()) return;
        ReexecElevated(RemoveArg);
    }

    public static int RunAddInElevatedChild()
    {
        if (IsBypassActive()) return 0;
        try
        {
            File.AppendAllText(HostsPath, Environment.NewLine + "127.0.0.1 " + MarkerHost + " # VRCResolver" + Environment.NewLine);
            return 0;
        }
        catch (Exception ex)
        {
            ConsoleUx.Error(LogComponent.Hosts, "add failed: " + ex.Message);
            return 1;
        }
    }

    public static int RunRemoveInElevatedChild()
    {
        if (!File.Exists(HostsPath)) return 0;
        try
        {
            var lines = File.ReadAllLines(HostsPath);
            var kept = new List<string>(lines.Length);
            int removed = 0;
            foreach (var l in lines)
            {
                if (LineIsBypassEntry(l)) { removed++; continue; }
                kept.Add(l);
            }
            if (removed == 0) return 0;
            File.WriteAllLines(HostsPath, kept);
            return 0;
        }
        catch (Exception ex)
        {
            ConsoleUx.Error(LogComponent.Hosts, "remove failed: " + ex.Message);
            return 1;
        }
    }

    private static bool ReexecElevated(string arg)
    {
        string? exe = Process.GetCurrentProcess().MainModule?.FileName;
        if (string.IsNullOrEmpty(exe)) return false;
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = exe,
                Arguments = arg,
                UseShellExecute = true,
                Verb = "runas",
                WindowStyle = ProcessWindowStyle.Hidden
            };
            using var proc = Process.Start(psi);
            proc?.WaitForExit(60000);
            if (proc != null && !proc.HasExited)
            {
                ConsoleUx.Warn(LogComponent.Hosts, "elevation child still running after 60s -- continuing without hosts entry.");
            }
            return true;
        }
        catch (Win32Exception)
        {
            ConsoleUx.Write(LogComponent.Hosts, "UAC declined -- entry not modified.");
            return false;
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Hosts, "elevation error: " + ex.Message);
            return false;
        }
    }
}
