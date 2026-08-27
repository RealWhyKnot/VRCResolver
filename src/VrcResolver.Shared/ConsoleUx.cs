using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Threading;

namespace VrcResolver.Shared;

public enum LogComponent
{
    Mesh,
    Ipc,
    Hosts,
    Patch,
    Wrapper,
    Heartbeat,
    Relay,
    Terminal,
    Codec,
    Report,
    Update,
    VrcLog,
    YtDlp,
    Shutdown,
}

public enum ResolveStatus
{
    Resolved,
    Fallback,
    Failed,
    Unexpected,
}

public static class ConsoleUx
{
    private static readonly object s_lock = new();
    private static IConsoleOverlay? s_overlay;

    private static ConsoleColor ColorFor(LogComponent c) => c switch
    {
        LogComponent.Mesh => ConsoleColor.DarkCyan,
        LogComponent.Ipc => ConsoleColor.DarkGray,
        LogComponent.Hosts => ConsoleColor.DarkGray,
        LogComponent.Patch => ConsoleColor.Gray,
        LogComponent.Wrapper => ConsoleColor.DarkGray,
        LogComponent.Heartbeat => ConsoleColor.DarkGray,
        LogComponent.Relay => ConsoleColor.Gray,
        LogComponent.Terminal => ConsoleColor.Gray,
        LogComponent.Codec => ConsoleColor.Gray,
        LogComponent.Report => ConsoleColor.DarkGray,
        LogComponent.Update => ConsoleColor.Gray,
        LogComponent.VrcLog => ConsoleColor.DarkGray,
        LogComponent.YtDlp => ConsoleColor.Gray,
        LogComponent.Shutdown => ConsoleColor.DarkRed,
        _ => ConsoleColor.Gray,
    };

    private static string Tag(LogComponent c) => c switch
    {
        LogComponent.Mesh => "[mesh]",
        LogComponent.Ipc => "[ipc]",
        LogComponent.Hosts => "[hosts]",
        LogComponent.Patch => "[patch]",
        LogComponent.Wrapper => "[wrapper]",
        LogComponent.Heartbeat => "[heartbeat]",
        LogComponent.Relay => "[relay]",
        LogComponent.Terminal => "[terminal]",
        LogComponent.Codec => "[codec]",
        LogComponent.Report => "[report]",
        LogComponent.Update => "[update]",
        LogComponent.VrcLog => "[vrclog]",
        LogComponent.YtDlp => "[yt-dlp]",
        LogComponent.Shutdown => "[shutdown]",
        _ => "[?]",
    };

    public static IDisposable UseOverlay(IConsoleOverlay overlay)
    {
        if (overlay == null) throw new ArgumentNullException(nameof(overlay));
        lock (s_lock)
        {
            TryClearOverlayLocked();
            s_overlay = overlay;
            TryRenderOverlayLocked();
        }
        return new OverlayRegistration(overlay);
    }

    public static void WithConsoleLock(Action action)
    {
        if (action == null) throw new ArgumentNullException(nameof(action));
        lock (s_lock) action();
    }

    public static void Write(LogComponent c, string body)
    {
        WriteStamped(ColorFor(c), $"{Bullet()} {Tag(c)} {body}");
    }

    public static void Success(LogComponent c, string body)
    {
        WriteStamped(ConsoleColor.Green, $"{Bullet()} {Tag(c)} {body}");
    }

    public static void Warn(LogComponent c, string body)
    {
        WriteStamped(ConsoleColor.Yellow, $"{Bullet()} {Tag(c)}[warn] {body}");
    }

    public static void Error(LogComponent c, string body)
    {
        WriteStamped(ConsoleColor.Red, $"{Bullet()} {Tag(c)}[err] {body}");
    }

    public static void Fatal(string body)
    {
        WriteStamped(ConsoleColor.Red, $"{Bullet()} [FATAL] {body}");
    }

    private const int HostWidth = 28;
    private const int PlayerWidth = 14;
    private const int OutcomeWidth = 9;

    public static void ResolveOutcome(
        string host,
        string player,
        ResolveStatus status,
        bool viaCache,
        bool viaLhYt,
        TimeSpan elapsed,
        string? reason)
    {
        ConsoleColor color;
        string token;
        string word;
        switch (status)
        {
            case ResolveStatus.Resolved:
                color = ConsoleColor.Green;
                token = "OK";
                word = viaCache ? "cached" : "resolved";
                break;
            case ResolveStatus.Fallback:
                color = ConsoleColor.Yellow;
                token = "!!";
                word = "fallback";
                break;
            case ResolveStatus.Failed:
                color = ConsoleColor.Red;
                token = "XX";
                word = "failed";
                break;
            default:
                color = ConsoleColor.DarkGray;
                token = "??";
                word = "unknown";
                break;
        }

        string hostCol = TruncatePad(host, HostWidth);
        string playerCol = TruncatePad(player, PlayerWidth);
        string ytTag = viaLhYt ? " [yt]" : "     ";
        string elapsedStr = FormatElapsed(elapsed);
        string reasonSuffix = string.IsNullOrEmpty(reason) ? "" : "   " + reason;

        string line = string.Format(
            CultureInfo.InvariantCulture,
            "{0} [resolve] {1}  {2,-" + OutcomeWidth + "} {3}{4}  {5}  {6,6}{7}",
            Bullet(), token, word, hostCol, ytTag, playerCol, elapsedStr, reasonSuffix);

        WriteStamped(color, line);
    }

    public static void WrapperFallback(string host, string reason, long elapsedMs)
    {
        string line = string.Format(
            CultureInfo.InvariantCulture,
            "{0} [wrapper] !!  {1,-" + OutcomeWidth + "} {2}       {3}   elapsed={4}ms",
            Bullet(), "fallback", TruncatePad(host, HostWidth), reason, elapsedMs);
        WriteStamped(ConsoleColor.Yellow, line);
    }

    public static void Banner(
        string version,
        string sha,
        string buildTime,
        bool isDev,
        IReadOnlyList<(string Label, string Value)> paths)
    {
        const string divider = "============================================================";
        const string subdivider = "------------------------------------------------------------";

        lock (s_lock)
        {
            WriteRaw(ConsoleColor.DarkGray, divider);
            WriteRaw(ConsoleColor.White, $"  VRCResolver {version}");
            WriteRaw(ConsoleColor.Gray, "  local video relay for VRChat");
            WriteRaw(ConsoleColor.Gray, $"  sha {sha}  built {buildTime}");
            if (isDev)
            {
                WriteRaw(ConsoleColor.Yellow, "  mode: DEV (diagnostic logs mirrored to console; relay trace enabled)");
            }
            WriteRaw(ConsoleColor.DarkGray, subdivider);

            int labelWidth = paths.Max(p => p.Label.Length);
            foreach (var (label, value) in paths)
            {
                string padded = (label + ":").PadRight(labelWidth + 2);
                WriteRaw(ConsoleColor.Gray, $"  {padded} {value}");
            }
            WriteRaw(ConsoleColor.DarkGray, divider);
            WriteRaw(ConsoleColor.Gray, "");
        }
    }

    public static void PhaseDivider(string label)
    {
        const int width = 60;
        string body = " " + label + " ";
        int pad = Math.Max(0, width - body.Length);
        int left = pad / 2;
        int right = pad - left;
        string line = new string('-', left) + body + new string('-', right);
        WriteStamped(ConsoleColor.DarkGray, line);
    }

    private static string TruncatePad(string s, int width)
    {
        if (s == null) s = "";
        if (s.Length > width)
        {
            return width >= 4 ? s.Substring(0, width - 2) + ".." : s.Substring(0, width);
        }
        return s.PadRight(width);
    }

    private static string FormatElapsed(TimeSpan t)
    {
        double seconds = t.TotalSeconds;
        if (seconds < 60.0)
        {
            return seconds.ToString("0.0", CultureInfo.InvariantCulture) + "s";
        }
        int m = (int)(seconds / 60.0);
        int s = (int)(seconds - m * 60.0);
        return m + "m" + s + "s";
    }

    private static void WriteStamped(ConsoleColor color, string body)
    {
        string stamped = "[" + DateTime.Now.ToString("HH:mm:ss", CultureInfo.InvariantCulture) + "] " + body;
        WriteRaw(color, stamped);
    }

    private static void WriteRaw(ConsoleColor color, string text)
    {
        lock (s_lock)
        {
            TryClearOverlayLocked();
            ConsoleColor prev;
            try { prev = Console.ForegroundColor; }
            catch { prev = ConsoleColor.Gray; }
            try
            {
                if (ShouldUseColor())
                    try { Console.ForegroundColor = color; } catch { }
                Console.WriteLine(text);
            }
            finally
            {
                try { Console.ForegroundColor = prev; } catch { }
            }
            TryRenderOverlayLocked();
        }
    }

    private static void TryClearOverlayLocked()
    {
        try { s_overlay?.ClearLocked(); }
        catch { s_overlay = null; }
    }

    private static void TryRenderOverlayLocked()
    {
        try { s_overlay?.RenderLocked(); }
        catch { s_overlay = null; }
    }

    public static bool UseColor()
    {
        if (Console.IsOutputRedirected) return false;
        return string.IsNullOrEmpty(Environment.GetEnvironmentVariable("NO_COLOR"));
    }

    private static bool ShouldUseColor() => UseColor();

    private static string Bullet()
    {
        return ShouldUseUnicode() ? "•" : "*";
    }

    public static bool UseUnicode()
    {
        if (Console.IsOutputRedirected) return false;
        string? ascii = LegacyCompat.GetEnvWithLegacyFallback("ASCII_TERMINAL");
        if (string.Equals(ascii, "1", StringComparison.Ordinal)
            || string.Equals(ascii, "true", StringComparison.OrdinalIgnoreCase))
            return false;

        try
        {
            return Console.OutputEncoding.WebName.Contains("utf", StringComparison.OrdinalIgnoreCase);
        }
        catch
        {
            return true;
        }
    }

    private static bool ShouldUseUnicode() => UseUnicode();

    private sealed class OverlayRegistration : IDisposable
    {
        private readonly IConsoleOverlay _overlay;
        private int _disposed;

        public OverlayRegistration(IConsoleOverlay overlay)
        {
            _overlay = overlay;
        }

        public void Dispose()
        {
            if (Interlocked.Exchange(ref _disposed, 1) != 0) return;
            lock (s_lock)
            {
                if (!ReferenceEquals(s_overlay, _overlay)) return;
                TryClearOverlayLocked();
                s_overlay = null;
            }
        }
    }
}

public interface IConsoleOverlay
{
    void ClearLocked();
    void RenderLocked();
}
