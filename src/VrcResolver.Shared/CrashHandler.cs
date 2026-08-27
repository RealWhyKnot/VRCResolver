using System.Reflection;

namespace VrcResolver.Shared;

public static class CrashHandler
{
    private static readonly object _writeLock = new();
    private static string? _logDir;
    private static string _component = "unknown";
    private static int _installed;
    private static Func<string>? _stateSnapshot;

    public static void SetStateSnapshot(Func<string>? snapshot)
    {
        _stateSnapshot = snapshot;
    }

    public static void Install(string component)
    {
        if (Interlocked.Exchange(ref _installed, 1) != 0) return;
        _component = component;

        try
        {
            _logDir = AppPaths.CrashesDir();
            Directory.CreateDirectory(_logDir);
        }
        catch
        {
            _logDir = null;
        }

        AppDomain.CurrentDomain.UnhandledException += (_, e) =>
        {
            WriteCrashLog("UnhandledException", e.ExceptionObject as Exception, e.IsTerminating);
        };

        TaskScheduler.UnobservedTaskException += (_, e) =>
        {
            WriteCrashLog("UnobservedTaskException", e.Exception, terminating: false);
            e.SetObserved();
        };
    }

    private static string Redact(string s)
    {
        if (string.IsNullOrEmpty(s)) return s;
        try
        {
            string? userProfile = Environment.GetEnvironmentVariable("USERPROFILE");
            if (!string.IsNullOrEmpty(userProfile))
                s = s.Replace(userProfile, "%USERPROFILE%", StringComparison.OrdinalIgnoreCase);
        }
        catch { }
        try
        {
            string userName = Environment.UserName;
            if (userName.Length >= 3)
                s = s.Replace(userName, "<user>", StringComparison.OrdinalIgnoreCase);
        }
        catch { }
        return s;
    }

    private static void WriteCrashLog(string kind, Exception? ex, bool terminating)
    {
        if (_logDir == null) return;
        try
        {
            string ts = DateTime.UtcNow.ToString("yyyyMMdd-HHmmss-fff");
            string path = Path.Combine(_logDir, $"crash-{_component}-{ts}.log");
            if (!Monitor.TryEnter(_writeLock, TimeSpan.FromSeconds(1))) return;
            try
            {
                using var w = new StreamWriter(path, append: false);
                w.WriteLine("=== vrcresolver crash log ===");
                w.WriteLine($"timestamp:    {DateTime.UtcNow:o}");
                w.WriteLine($"component:    {_component}");
                w.WriteLine($"kind:         {kind}");
                w.WriteLine($"terminating:  {terminating}");
                w.WriteLine($"pid:          {Environment.ProcessId}");
                w.WriteLine($"version:      {Assembly.GetEntryAssembly()?.GetName().Version}");
                w.WriteLine($"basedir:      {Redact(AppContext.BaseDirectory)}");
                w.WriteLine($"os:           {Environment.OSVersion}");

                var snapshot = _stateSnapshot;
                if (snapshot != null)
                {
                    w.WriteLine();
                    w.WriteLine("--- state snapshot ---");
                    try { w.WriteLine(Redact(snapshot())); }
                    catch (Exception sex) { w.WriteLine("(snapshot delegate threw: " + sex.GetType().Name + ": " + sex.Message + ")"); }
                }

                w.WriteLine();
                w.WriteLine(Redact(ex?.ToString() ?? "(no exception object)"));
            }
            finally
            {
                Monitor.Exit(_writeLock);
            }
        }
        catch
        {
        }
    }
}
