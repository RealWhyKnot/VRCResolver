using System.Globalization;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed class TerminalRenderer
{
    private readonly Func<WatchdogActivitySnapshot> _snapshot;
    private readonly Func<WatchdogBandwidthSnapshot> _bandwidth;
    private readonly Func<bool> _meshConnected;
    private readonly Func<int> _spinnerIndex;
    private readonly Func<string> _input;
    private readonly Func<AppSettings> _settings;
    private readonly Func<bool> _animationsAvailable;
    private readonly Func<bool> _unicodeAvailable;
    private readonly Action<string, string>? _recordOutput;
    private readonly Overlay _overlay;
    private IDisposable? _overlayRegistration;
    private bool _cursorWasVisible = true;
    private bool _cursorHidden;

    public TerminalRenderer(
        Func<WatchdogActivitySnapshot> snapshot,
        Func<WatchdogBandwidthSnapshot>? bandwidth,
        Func<bool> meshConnected,
        Func<int> spinnerIndex,
        Func<string> input,
        Func<AppSettings>? settings = null,
        Func<bool>? animationsAvailable = null,
        Func<bool>? unicodeAvailable = null,
        Action<string, string>? recordOutput = null)
    {
        _snapshot = snapshot ?? throw new ArgumentNullException(nameof(snapshot));
        _bandwidth = bandwidth ?? WatchdogStats.GetBandwidthSnapshot;
        _meshConnected = meshConnected ?? throw new ArgumentNullException(nameof(meshConnected));
        _spinnerIndex = spinnerIndex ?? throw new ArgumentNullException(nameof(spinnerIndex));
        _input = input ?? throw new ArgumentNullException(nameof(input));
        _settings = settings ?? AppSettingsStore.Shared.Snapshot;
        _animationsAvailable = animationsAvailable ?? TerminalCapabilities.UseAnimations;
        _unicodeAvailable = unicodeAvailable ?? TerminalCapabilities.UseUnicode;
        _recordOutput = recordOutput;
        _overlay = new Overlay(this);
    }

    public void AttachOverlay()
    {
        _overlayRegistration ??= ConsoleUx.UseOverlay(_overlay);
        _cursorHidden = TerminalCapabilities.TrySetCursorVisible(false, out _cursorWasVisible);
    }

    public void DetachOverlay()
    {
        _overlayRegistration?.Dispose();
        _overlayRegistration = null;
        if (_cursorHidden)
            TerminalCapabilities.RestoreCursorVisible(_cursorWasVisible);
        _cursorHidden = false;
    }

    public void RenderOverlay()
    {
        ConsoleUx.WithConsoleLock(_overlay.RenderLocked);
    }

    public bool ShouldUseFastRefresh()
    {
        try
        {
            return TerminalRefreshPolicy.ShouldUseFastRefresh(
                _snapshot(),
                DateTime.UtcNow,
                _settings(),
                _animationsAvailable());
        }
        catch
        {
            return false;
        }
    }

    public void EchoCommand(string command)
    {
        ConsoleUx.WithConsoleLock(() =>
        {
            _overlay.ClearLocked();
            WriteFrameLine(TerminalFrame.FromRuns(
                new TerminalTextRun("vrcr> ", ConsoleColor.White),
                new TerminalTextRun(command ?? "", ConsoleColor.Gray)));
            _overlay.RenderLocked();
        });
    }

    public void Info(string message)
    {
        _recordOutput?.Invoke("info", message);
        ConsoleUx.Write(LogComponent.Terminal, message);
    }

    public void Success(string message)
    {
        _recordOutput?.Invoke("success", message);
        ConsoleUx.Success(LogComponent.Terminal, message);
    }

    public void Warn(string message)
    {
        _recordOutput?.Invoke("warn", message);
        ConsoleUx.Warn(LogComponent.Terminal, message);
    }

    public void Error(string message)
    {
        _recordOutput?.Invoke("error", message);
        ConsoleUx.Error(LogComponent.Terminal, message);
    }

    public void ClearScreen()
    {
        ConsoleUx.WithConsoleLock(() =>
        {
            _overlay.ClearLocked();
            try { Console.Clear(); }
            catch { /* non-clearable host */ }
            _overlay.RenderLocked();
        });
    }

    public void RenderHelp(IReadOnlyList<TerminalCommand> commands)
    {
        RenderFrames("commands", TerminalBlocks.CommandPalette(commands, UsableWidth(), Glyphs()));
    }

    public void RenderCompletions(string input, IReadOnlyList<TerminalCompletionItem> items)
    {
        var frames = TerminalBlocks.CompletionPalette(items, UsableWidth(), Glyphs());
        _recordOutput?.Invoke("matches", frames.Count == 0 ? "" : frames[0].PlainText);
        ConsoleUx.WithConsoleLock(() =>
        {
            _overlay.ClearLocked();
            WriteFrameLine(TerminalFrame.FromRuns(
                new TerminalTextRun("vrcr> ", ConsoleColor.White),
                new TerminalTextRun(input ?? "", ConsoleColor.Gray)));
            foreach (TerminalFrame frame in frames)
                WriteFrameLine(frame);
            _overlay.RenderLocked();
        });
    }

    public void RenderHistory(IReadOnlyList<string> commands)
    {
        if (commands.Count == 0)
        {
            Info("history: no commands in this session yet.");
            return;
        }

        WriteBlock("history", commands.Select((cmd, index) =>
            ((index + 1).ToString(CultureInfo.InvariantCulture).PadLeft(2), cmd)));
    }

    public void RenderTools()
    {
        WriteTableBlock("managed subsystems", new[]
        {
            ("VRChat hook", "ready", "keeps game video requests flowing through VRCResolver"),
            ("resolver mesh", _meshConnected() ? "online" : "reconnecting", "resolves playback URLs and fallback decisions"),
            ("local video", "ready", "serves localhost.youtube.com playback to the game"),
            ("hosts entry", "managed", "keeps localhost.youtube.com pointing at this PC"),
            ("diagnostics", "recording", "writes logs, crash snapshots, and playback feedback"),
        });
    }

    public void RenderDiagnostics(TerminalSessionStore session)
    {
        var frames = new List<TerminalFrame>();
        frames.AddRange(TerminalBlocks.KeyValue("diagnostics", new[]
        {
            ("install", AppContext.BaseDirectory),
            ("state", AppPaths.StateRoot()),
            ("settings", AppSettingsStore.Shared.FilePath),
            ("logs", AppPaths.LogsDir()),
            ("crashes", AppPaths.CrashesDir()),
            ("terminal history", session.HistoryPath),
            ("this session", session.SessionPath),
        }, UsableWidth(), Glyphs()));

        RenderFrames("diagnostics", frames);
    }

    public void RenderStatus(WatchdogActivitySnapshot snapshot, bool meshConnected)
    {
        AppSettings settings = _settings();
        WatchdogBandwidthSnapshot bandwidth = _bandwidth();
        DateTime now = DateTime.UtcNow;
        bool relayActive = snapshot.RelayActive(now, TerminalRefreshPolicy.RecentActivityWindow);
        bool upstreamActive = snapshot.UpstreamActive(now, TerminalRefreshPolicy.RecentActivityWindow);

        string videoState = relayActive ? "serving VRChat" : "waiting";
        string videoDetail = relayActive
            ? WatchdogDisplay.FormatBytesPerSecond(bandwidth.CurrentBytesPerSecond) + " now, "
                + WatchdogDisplay.FormatBytes(snapshot.RelayBytesTotal) + " served"
            : WatchdogDisplay.FormatBytes(snapshot.RelayBytesTotal) + " served total";

        string upstreamState = meshConnected
            ? (upstreamActive ? "pulling media" : "online idle")
            : "reconnecting";
        string upstreamDetail = snapshot.UpstreamRelayBytesTotal > 0
            ? WatchdogDisplay.FormatBytes(snapshot.UpstreamRelayBytesTotal) + " received"
            : "no media pulled yet";

        WritePanel("status", new[]
        {
            ("local video", videoState, videoDetail),
            ("resolver", upstreamState, upstreamDetail),
            ("resolves", snapshot.ResolvesTotal.ToString(CultureInfo.InvariantCulture) + " total",
                snapshot.ResolvesViaLhYt.ToString(CultureInfo.InvariantCulture) + " local video, "
                + snapshot.ResolvesCacheHits.ToString(CultureInfo.InvariantCulture) + " cache hits"),
            ("last video", DescribeAge(snapshot.LastRelayUtc, now), "last bytes served to VRChat"),
            ("last pull", DescribeAge(snapshot.LastUpstreamRelayUtc, now), "last bytes received from the resolver"),
            ("peak speed", bandwidth.HasTraffic ? WatchdogDisplay.FormatBytesPerSecond(bandwidth.PeakBytesPerSecond) : "none yet",
                TerminalEffectEngine.Sparkline(bandwidth.HistoryBytesPerSecond, 12, Glyphs())),
            ("terminal", settings.Terminal.StatusLine ? "live prompt" : "prompt only", DescribeTerminal(settings)),
        });
    }

    public void RenderSettings(IReadOnlyList<AppSettingDefinition> settings, AppSettings snapshot)
    {
        WriteTableBlock("settings", settings.Select(s =>
        {
            string value = s.Get(snapshot);
            string suffix = s.RestartRequired ? " (restart)" : "";
            return (s.Key, value + suffix, s.Description);
        }));
    }

    public void RenderSetting(AppSettingDefinition setting, AppSettings snapshot)
    {
        string suffix = setting.RestartRequired ? " (takes effect next launch)" : "";
        WriteBlock("setting", new[]
        {
            ("name", setting.Key),
            ("current", setting.Get(snapshot) + suffix),
            ("enter", string.Join(", ", setting.Choices)),
            ("about", setting.Description),
        });
    }

    public void RenderSettingsHelp()
    {
        WriteBlock("settings help", new[]
        {
            ("settings", "list settings"),
            ("settings get <name>", "show one setting"),
            ("settings set <name> <value>", "change a setting"),
            ("settings <name> <value>", "change a setting"),
            ("settings reset <name>", "reset one setting"),
            ("settings reset all", "reset every setting"),
            ("example", "settings status-line off"),
            ("example", "settings video-support-updates on"),
        });
    }

    private void WriteBlock(string title, IEnumerable<(string Left, string Right)> rows)
    {
        RenderFrames("block", TerminalBlocks.KeyValue(title, rows, UsableWidth(), Glyphs()));
    }

    private void WriteTableBlock(string title, IEnumerable<(string Name, string Value, string Description)> rows)
    {
        RenderFrames("table", TerminalBlocks.Table(title, rows, UsableWidth(), Glyphs()));
    }

    private void WritePanel(string title, IEnumerable<(string Name, string State, string Detail)> rows)
    {
        RenderFrames("panel", TerminalBlocks.Panel(title, rows, UsableWidth(), Glyphs()));
    }

    private void RenderFrames(string kind, IReadOnlyList<TerminalFrame> frames)
    {
        _recordOutput?.Invoke(kind, frames.Count == 0 ? "" : frames[0].PlainText);
        ConsoleUx.WithConsoleLock(() =>
        {
            _overlay.ClearLocked();
            foreach (TerminalFrame frame in frames)
                WriteFrameLine(frame);
            _overlay.RenderLocked();
        });
    }

    private TerminalGlyphSet Glyphs()
    {
        return TerminalGlyphSet.For(_unicodeAvailable());
    }

    private static int UsableWidth()
    {
        return Math.Max(20, ConsoleWidth() - 2);
    }

    private static string DescribeAge(DateTime? timestampUtc, DateTime nowUtc)
    {
        if (!timestampUtc.HasValue) return "not yet";
        TimeSpan age = nowUtc - timestampUtc.Value;
        if (age < TimeSpan.Zero) age = TimeSpan.Zero;
        if (age.TotalSeconds < 2) return "now";
        if (age.TotalSeconds < 60) return ((int)age.TotalSeconds).ToString(CultureInfo.InvariantCulture) + "s ago";
        if (age.TotalMinutes < 60) return ((int)age.TotalMinutes).ToString(CultureInfo.InvariantCulture) + "m ago";
        return ((int)Math.Min(age.TotalHours, 99)).ToString(CultureInfo.InvariantCulture) + "h ago";
    }

    private static string DescribeTerminal(AppSettings settings)
    {
        if (!settings.Terminal.StatusLine)
            return "status line off";

        return settings.Terminal.Animations
            ? "status line on, motion on"
            : "status line on, motion off";
    }

    private static int ConsoleWidth()
    {
        try { return Math.Max(20, Console.WindowWidth - 1); }
        catch { return 119; }
    }

    private static void WriteFrameLine(TerminalFrame frame)
    {
        WriteRuns(Console.Out, frame.Runs);
        Console.Out.WriteLine();
    }

    internal static void WriteRuns(TextWriter writer, IReadOnlyList<TerminalTextRun> runs)
    {
        if (writer == null) throw new ArgumentNullException(nameof(writer));
        if (runs == null) throw new ArgumentNullException(nameof(runs));

        if (!TerminalCapabilities.UseColor())
        {
            foreach (TerminalTextRun run in runs)
                writer.Write(run.Text);
            return;
        }

        ConsoleColor prev;
        try { prev = Console.ForegroundColor; }
        catch { prev = ConsoleColor.Gray; }
        try
        {
            foreach (TerminalTextRun run in runs)
            {
                try { Console.ForegroundColor = run.Color; } catch { /* no-tty */ }
                writer.Write(run.Text);
            }
        }
        finally
        {
            try { Console.ForegroundColor = prev; } catch { /* no-tty */ }
        }
    }

    private sealed class Overlay : IConsoleOverlay
    {
        private readonly TerminalRenderer _owner;
        private readonly TerminalOverlayLine _line = new();

        public Overlay(TerminalRenderer owner)
        {
            _owner = owner;
        }

        public void ClearLocked()
        {
            _line.Clear(Console.Out);
        }

        public void RenderLocked()
        {
            AppSettings settings = _owner._settings();
            TerminalFrame frame = TerminalStatusFormatter.Format(
                _owner._snapshot(),
                _owner._bandwidth(),
                DateTime.UtcNow,
                _owner._meshConnected(),
                _owner._spinnerIndex(),
                ConsoleWidth(),
                _owner._input(),
                settings.Terminal.StatusLine,
                settings.Terminal.Animations && _owner._animationsAvailable(),
                _owner._unicodeAvailable());

            _line.RenderIfChanged(Console.Out, frame, WriteRuns);
        }
    }
}
