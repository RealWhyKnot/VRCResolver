using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

// Pins localhost.youtube.com → 127.0.0.1 in %WINDIR%\System32\drivers\etc\hosts.
// VRChat's AVPro trusted-host allowlist matches `*.youtube.com`, so a
// resolved URL whose host is `localhost.youtube.com` plays in public worlds
// where AVPro otherwise rejects it. (Currently the consumer that decoded
// these wrapped URLs lives outside this repo; the hosts pin is kept so the
// mechanism is ready when restoration lands.)
//
// The watchdog re-adds the entry on a periodic tick (HostsTicker.Tick) if it
// goes missing — manual edits, OS rollback, antivirus rewrite. UAC re-prompt
// is rate-limited per HostsTicker so a user who declines doesn't get spammed.
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

    // Read the hosts file with the same FileShare.None probe pattern
    // PatchManager uses for yt-dlp.exe — antivirus or admin tools may be
    // briefly holding hosts open. Up to 3 retries with 200ms backoff. Returns
    // false on persistent failure (caller treats as "unknown" and skips the
    // tick rather than re-prompting UAC for nothing).
    public static bool TryReadBypassState(out bool present, out string? errorReason)
    {
        present = false;
        errorReason = null;
        if (!File.Exists(HostsPath)) { errorReason = "hosts file missing"; return true; /* missing == not present */ }
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
            catch (UnauthorizedAccessException ex) { lastEx = ex; break; /* DACL won't change on retry */ }
            catch (Exception ex) { lastEx = ex; break; }
            if (attempt < 3) Thread.Sleep(200);
        }
        errorReason = lastEx?.GetType().Name + ": " + (lastEx?.Message ?? "<unknown>");
        return false;
    }

    // Parse one hosts file line and decide whether it's our bypass entry.
    // Conservative — explicitly NOT a substring match. A comment line that
    // mentions "127.0.0.1 localhost.youtube.com" inside a `# ...` is not a
    // bypass entry; nor is `127.0.0.2 localhost.youtube.com` (wrong IP); nor
    // is a line where the host appears as part of a longer token like
    // `notlocalhost.youtube.com`. Token-aware: split on whitespace, ignore
    // any trailing `#` comment, first token must equal 127.0.0.1, any
    // subsequent token (case-insensitive) must equal `localhost.youtube.com`.
    public static bool LineIsBypassEntry(string? rawLine)
    {
        if (string.IsNullOrWhiteSpace(rawLine)) return false;
        string trimmed = rawLine.TrimStart();
        if (trimmed.Length == 0 || trimmed[0] == '#') return false;

        // Strip trailing `# comment` (ours is `# VRCResolver`; pre-rename installs wrote `# WKVRCProxy`, but also
        // tolerate hand-edited variants).
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
            // Idempotent: if nothing was filtered, skip the write entirely
            // so we don't churn the hosts file's mtime (which trips AV
            // file-watchers + Windows tampering monitors for no reason).
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
            // 60s timeout so a UAC dialog left open (user away from keyboard)
            // doesn't block startup forever (Tier D backlog item).
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
