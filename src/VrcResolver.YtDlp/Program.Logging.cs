using System.Reflection;
using System.Text;
using VrcResolver.Shared;

namespace VrcResolver.YtDlp;

internal static partial class Program
{
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
        catch { }
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
            s_logInitFailed = true;
            return null;
        }
    }

    private static void CloseLog()
    {
        lock (s_logLock)
        {
            try { s_logWriter?.Flush(); } catch { }
            try { s_logWriter?.Dispose(); } catch { }
            s_logWriter = null;
        }
    }
}
