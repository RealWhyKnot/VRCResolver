using System.Reflection;
using System.Text;
using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

internal static partial class Program
{
    // Truncate + escape a free-form string for inclusion in a single log
    // line. Newlines are converted to literal "\n" so a multi-line yt-dlp
    // stderr block doesn't fragment the log.
    private static string Preview(string s, int maxLen)
    {
        string trimmed = s.Length > maxLen ? s[..maxLen] + "...(truncated)" : s;
        return trimmed.Replace("\r", "").Replace("\n", "\\n");
    }

    private static void LogStartBanner(string[] args, string url, string? formatArg, string player)
    {
        string ver = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "?";
        var sb = new StringBuilder();
        sb.Append("START pid=").Append(Environment.ProcessId);
        sb.Append(" ver=").Append(ver);
        sb.Append(" argc=").Append(args.Length);
        sb.Append(" url-host=").Append(string.IsNullOrEmpty(url) ? "<none>" : LogUtil.BareHost(url));
        sb.Append(" player=").Append(player);
        sb.Append(" -f=").Append(formatArg ?? "<none>");
        // Args summary: drop any arg that's an absolute URL (host already
        // logged separately) and any arg that looks like a multi-K-char
        // host-allowlist (--exp-allow / --wild-allow) — those run into
        // thousands of chars and aren't useful in the per-line log.
        sb.Append(" flags=[");
        bool first = true;
        for (int i = 0; i < args.Length; i++)
        {
            string a = args[i];
            if (a.StartsWith("http://", StringComparison.OrdinalIgnoreCase)
                || a.StartsWith("https://", StringComparison.OrdinalIgnoreCase))
                continue;
            if ((a == "--exp-allow" || a == "--wild-allow") && i + 1 < args.Length)
            {
                if (!first) sb.Append(',');
                int hostCount = args[i + 1].Split(',').Length;
                sb.Append(a).Append("[~").Append(hostCount).Append(" hosts]");
                i++;
                first = false;
                continue;
            }
            if (!first) sb.Append(',');
            sb.Append(a.Length > 64 ? a[..64] + "..." : a);
            first = false;
        }
        sb.Append(']');
        Log(sb.ToString());
    }

    // Best-effort single-file diagnostic. Log lands at
    //   %LOCALAPPDATA%Low\vrcresolver\logs\yt-dlp-wrapper.log
    // — must live under LocalLow because the wrapper runs at Low integrity
    // (inherited from VRChat's Tools dir which sits in LocalLow). A
    // Low-integrity process cannot write to Medium-integrity dirs, so the
    // earlier %LOCALAPPDATA% path silently failed for every VRChat-invoked
    // call. Watchdog reads from this same LocalLow path so log surfaces
    // are unified across components. Failures are still swallowed — a
    // yt-dlp invocation that can't log shouldn't break the resolve pipeline.
    //
    // Single FileStream cached for the lifetime of the invocation (~10-15
    // Log calls per resolve). Earlier impl re-opened the file on every call
    // via File.AppendAllText + Directory.CreateDirectory, costing 5-50 ms
    // of avoidable I/O per resolve. Lazy-init on first call so a wrapper
    // run that never logs (impossible today, but cheap to handle) doesn't
    // touch disk. CloseLog() is invoked from Main's finally so the stream
    // flushes before process exit; an exit that bypasses the finally still
    // produces a useful tail because we Flush after every WriteLine.
    private static readonly object s_logLock = new();
    private static StreamWriter? s_logWriter;
    private static bool s_logInitFailed;

    private static void Log(string message)
    {
        try
        {
            string line = "[" + DateTime.UtcNow.ToString("o") + "] [" + s_rid + "] " + message;
            lock (s_logLock)
            {
                var w = s_logWriter ?? OpenLogWriter();
                if (w == null) return;
                w.WriteLine(line);
                w.Flush();
            }
        }
        catch { /* best-effort */ }
    }

    private static StreamWriter? OpenLogWriter()
    {
        if (s_logInitFailed) return null;
        try
        {
            string logDir = AppPaths.LogsDir();
            Directory.CreateDirectory(logDir);
            string logPath = Path.Combine(logDir, "yt-dlp-wrapper.log");
            var fs = new FileStream(logPath, FileMode.Append, FileAccess.Write, FileShare.ReadWrite);
            s_logWriter = new StreamWriter(fs, Encoding.UTF8) { AutoFlush = false, NewLine = "\n" };
            return s_logWriter;
        }
        catch
        {
            // Disk full / permissions / unexpected layout. Set the failure
            // flag so the next Log() doesn't keep retrying syscalls each
            // call — that's the exact loss the refactor is meant to avoid.
            s_logInitFailed = true;
            return null;
        }
    }

    private static void CloseLog()
    {
        lock (s_logLock)
        {
            try { s_logWriter?.Flush(); } catch { /* best-effort */ }
            try { s_logWriter?.Dispose(); } catch { /* best-effort */ }
            s_logWriter = null;
        }
    }
}
