using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed class TerminalSessionStore : IDisposable
{
    private const int MaxHistoryCommands = 200;
    private const int MaxRecentCommands = 30;
    private const long MaxHistoryBytes = 256 * 1024;
    private readonly object _lock = new();
    private readonly string _dir;
    private readonly string _historyPath;
    private readonly string _sessionPath;
    private readonly List<string> _recentCommands = new();
    private bool _stopped;

    public TerminalSessionStore(string dir, DateTime? nowUtc = null)
    {
        _dir = dir ?? throw new ArgumentNullException(nameof(dir));
        _historyPath = Path.Combine(_dir, "history.txt");
        DateTime stamp = nowUtc ?? DateTime.UtcNow;
        _sessionPath = Path.Combine(
            _dir,
            "session-" + stamp.ToString("yyyyMMdd-HHmmss") + ".jsonl");
    }

    public static TerminalSessionStore CreateDefault()
    {
        return new TerminalSessionStore(Path.Combine(AppPaths.StateRoot(), "terminal"));
    }

    public string DirectoryPath => _dir;
    public string HistoryPath => _historyPath;
    public string SessionPath => _sessionPath;

    public IReadOnlyList<string> RecentCommands
    {
        get
        {
            lock (_lock) return _recentCommands.ToArray();
        }
    }

    public IReadOnlyList<string> LoadHistory()
    {
        try
        {
            if (!File.Exists(_historyPath)) return Array.Empty<string>();
            var info = new FileInfo(_historyPath);
            if (info.Length > MaxHistoryBytes) return Array.Empty<string>();

            return File.ReadLines(_historyPath)
                .Where(s => !string.IsNullOrWhiteSpace(s))
                .Select(s => s.Trim())
                .TakeLast(MaxHistoryCommands)
                .ToArray();
        }
        catch
        {
            return Array.Empty<string>();
        }
    }

    public void Start()
    {
        Record(new TerminalSessionEvent
        {
            Type = "session_start",
            TimeUtc = DateTime.UtcNow,
            Text = "terminal session started",
        });
    }

    public void Stop()
    {
        lock (_lock)
        {
            if (_stopped) return;
            _stopped = true;
        }

        Record(new TerminalSessionEvent
        {
            Type = "session_end",
            TimeUtc = DateTime.UtcNow,
            Text = "terminal session ended",
        });
    }

    public void RecordCommand(string command)
    {
        command = (command ?? "").Trim();
        if (command.Length == 0) return;

        lock (_lock)
        {
            if (_recentCommands.Count == 0
                || !string.Equals(_recentCommands[^1], command, StringComparison.Ordinal))
            {
                _recentCommands.Add(command);
                if (_recentCommands.Count > MaxRecentCommands)
                    _recentCommands.RemoveRange(0, _recentCommands.Count - MaxRecentCommands);
            }
        }

        AppendHistory(command);
        Record(new TerminalSessionEvent
        {
            Type = "command",
            TimeUtc = DateTime.UtcNow,
            Command = command,
        });
    }

    public void RecordOutput(string level, string text)
    {
        Record(new TerminalSessionEvent
        {
            Type = string.IsNullOrWhiteSpace(level) ? "output" : "output:" + level.Trim().ToLowerInvariant(),
            TimeUtc = DateTime.UtcNow,
            Text = text,
        });
    }

    private void AppendHistory(string command)
    {
        try
        {
            Directory.CreateDirectory(_dir);
            File.AppendAllText(_historyPath, command + Environment.NewLine);
            TrimHistoryIfNeeded();
        }
        catch
        {
            // History is a convenience. Never let it affect watchdog behavior.
        }
    }

    private void TrimHistoryIfNeeded()
    {
        try
        {
            var info = new FileInfo(_historyPath);
            if (!info.Exists || info.Length <= MaxHistoryBytes) return;

            var lines = File.ReadLines(_historyPath)
                .Where(s => !string.IsNullOrWhiteSpace(s))
                .TakeLast(MaxHistoryCommands)
                .ToArray();
            File.WriteAllLines(_historyPath, lines);
        }
        catch
        {
            // Best-effort compaction only.
        }
    }

    private void Record(TerminalSessionEvent ev)
    {
        try
        {
            Directory.CreateDirectory(_dir);
            string line = JsonSerializer.Serialize(ev, MeshJsonContext.Default.TerminalSessionEvent);
            File.AppendAllText(_sessionPath, line + Environment.NewLine);
        }
        catch
        {
            // Session persistence must never break the watchdog.
        }
    }

    public void Dispose()
    {
        Stop();
    }
}

internal sealed class TerminalSessionEvent
{
    [JsonPropertyName("type")]
    public string Type { get; set; } = "";

    [JsonPropertyName("time_utc")]
    public DateTime TimeUtc { get; set; }

    [JsonPropertyName("command")]
    public string? Command { get; set; }

    [JsonPropertyName("text")]
    public string? Text { get; set; }
}
