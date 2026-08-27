using System.Text;

namespace VrcResolver.Shared;

public static class Logger
{
    private const long MaxBytes = 10L * 1024 * 1024;
    private static readonly TimeSpan RetentionWindow = TimeSpan.FromDays(7);

    private static readonly object _lock = new();
    private static StreamWriter? _writer;
    private static string? _logDir;
    private static string _component = "unknown";
    private static int _installed;
    private static int _devConsoleDiagnostics;

    private static long _lastWriteTicksUtc;
    public static DateTime LastWriteUtc => new(Volatile.Read(ref _lastWriteTicksUtc), DateTimeKind.Utc);

    public static bool DevConsoleDiagnosticsEnabled => Volatile.Read(ref _devConsoleDiagnostics) != 0;

    public static void SetDevConsoleDiagnostics(bool enabled)
    {
        Volatile.Write(ref _devConsoleDiagnostics, enabled ? 1 : 0);
    }

    public static void Install(string component)
    {
        if (Interlocked.Exchange(ref _installed, 1) != 0) return;
        _component = component;
        try
        {
            _logDir = AppPaths.LogsDir();
            Directory.CreateDirectory(_logDir);
            PruneOld();
            OpenNew();
        }
        catch
        {
            _logDir = null;
            return;
        }

        Console.SetOut(new TeeWriter(Console.Out));
        Console.SetError(new TeeWriter(Console.Error));
    }

    public static void Close()
    {
        lock (_lock)
        {
            try { _writer?.Flush(); } catch { }
            try { _writer?.Dispose(); } catch { }
            _writer = null;
        }
    }

    public static void WriteFileOnly(string? line)
    {
        Tee(line);
    }

    public static void WriteDiagnostic(LogComponent component, string fileLine, string consoleBody)
    {
        if (DevConsoleDiagnosticsEnabled)
        {
            ConsoleUx.Write(component, consoleBody);
            return;
        }

        Tee(fileLine);
    }

    public static void WarnDiagnostic(LogComponent component, string fileLine, string consoleBody)
    {
        if (DevConsoleDiagnosticsEnabled)
        {
            ConsoleUx.Warn(component, consoleBody);
            return;
        }

        Tee(fileLine);
    }

    private static void Tee(string? line)
    {
        Volatile.Write(ref _lastWriteTicksUtc, DateTime.UtcNow.Ticks);
        var w = _writer;
        if (w == null) return;
        lock (_lock)
        {
            w = _writer;
            if (w == null) return;
            try
            {
                w.Write(DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.fffZ "));
                w.WriteLine(line ?? "");
                w.Flush();
                if (w.BaseStream.Length >= MaxBytes)
                {
                    OpenNew();
                }
            }
            catch
            {
            }
        }
    }

    private static void OpenNew()
    {
        try { _writer?.Dispose(); } catch { }
        _writer = null;
        if (_logDir == null) return;
        string ts = DateTime.UtcNow.ToString("yyyyMMdd-HHmmss-fff");
        string path = Path.Combine(_logDir, $"{_component}-{ts}.log");
        var fs = new FileStream(path, FileMode.Append, FileAccess.Write, FileShare.Read);
        _writer = new StreamWriter(fs) { AutoFlush = false };
    }

    private static void PruneOld()
    {
        if (_logDir == null) return;
        var cutoff = DateTime.UtcNow - RetentionWindow;
        try
        {
            foreach (var f in Directory.EnumerateFiles(_logDir, $"{_component}-*.log"))
            {
                try { if (File.GetLastWriteTimeUtc(f) < cutoff) File.Delete(f); }
                catch { }
            }
        }
        catch { }
    }

    private sealed class TeeWriter : TextWriter
    {
        private readonly TextWriter _primary;
        public TeeWriter(TextWriter primary) { _primary = primary; }
        public override Encoding Encoding => _primary.Encoding;
        public override void WriteLine() { _primary.WriteLine(); Tee(""); }
        public override void WriteLine(string? value) { _primary.WriteLine(value); Tee(value); }
        public override void Write(string? value) { _primary.Write(value); }
        public override void Write(char value) { _primary.Write(value); }
        public override void Flush() { _primary.Flush(); }
    }
}
