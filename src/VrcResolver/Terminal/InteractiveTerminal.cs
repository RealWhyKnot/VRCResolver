using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed class InteractiveTerminal : IDisposable
{
    private readonly Action _requestShutdown;
    private readonly Func<bool> _meshConnected;
    private readonly CancellationTokenSource _cts = new();
    private readonly TerminalInputBuffer _input;
    private readonly TerminalCommandRegistry _commands;
    private readonly TerminalSessionStore _session;
    private readonly TerminalRenderer _renderer;

    private Task? _inputTask;
    private Task? _renderTask;
    private int _spinnerIndex;
    private int _started;
    private string _lastSuggestionInput = "";
    private volatile string _ghost = "";
    private volatile bool _stopped;

    public InteractiveTerminal(Action requestShutdown, Func<bool> meshConnected)
        : this(
            requestShutdown,
            meshConnected,
            TerminalSessionStore.CreateDefault())
    {
    }

    internal InteractiveTerminal(
        Action requestShutdown,
        Func<bool> meshConnected,
        TerminalSessionStore session)
    {
        _requestShutdown = requestShutdown ?? throw new ArgumentNullException(nameof(requestShutdown));
        _meshConnected = meshConnected ?? throw new ArgumentNullException(nameof(meshConnected));
        _session = session ?? throw new ArgumentNullException(nameof(session));
        _input = new TerminalInputBuffer(_session.LoadHistory());
        _commands = TerminalCommandRegistry.CreateDefault();
        _renderer = new TerminalRenderer(
            snapshot: WatchdogStats.GetActivitySnapshot,
            bandwidth: WatchdogStats.GetBandwidthSnapshot,
            meshConnected: MeshConnected,
            spinnerIndex: () => Volatile.Read(ref _spinnerIndex),
            input: _input.Text,
            settings: AppSettingsStore.Shared.Snapshot,
            recordOutput: _session.RecordOutput,
            inputCursor: () => _input.Cursor,
            inputGhost: () => _ghost);
    }

    public void Start()
    {
        if (Interlocked.Exchange(ref _started, 1) != 0) return;
        if (!CanUseInteractiveConsole()) return;

        _session.Start();
        _renderer.AttachOverlay();
        _renderer.Success("interactive terminal ready; type /help for commands.");
        _inputTask = Task.Run(() => InputLoopAsync(_cts.Token));
        _renderTask = Task.Run(() => RenderLoopAsync(_cts.Token));
    }

    public async Task StopAsync()
    {
        _stopped = true;
        _cts.Cancel();
        _renderer.DetachOverlay();
        _session.Stop();
        await AwaitNoThrowAsync(_renderTask, 500).ConfigureAwait(false);
        await AwaitNoThrowAsync(_inputTask, 500).ConfigureAwait(false);
    }

    private static bool CanUseInteractiveConsole()
    {
        if (!Environment.UserInteractive) return false;
        if (Console.IsInputRedirected || Console.IsOutputRedirected) return false;
        string? disabled = LegacyCompat.GetEnvWithLegacyFallback("NO_INTERACTIVE_TERMINAL");
        return !string.Equals(disabled, "1", StringComparison.Ordinal)
            && !string.Equals(disabled, "true", StringComparison.OrdinalIgnoreCase);
    }

    private async Task InputLoopAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            try
            {
                if (!Console.KeyAvailable)
                {
                    await Task.Delay(50, ct).ConfigureAwait(false);
                    continue;
                }

                await HandleKeyAsync(Console.ReadKey(intercept: true), ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException) { return; }
            catch (InvalidOperationException) { return; }
            catch (IOException) { return; }
            catch (Exception ex)
            {
                _renderer.Warn("input disabled: " + ex.GetType().Name + ": " + ex.Message);
                return;
            }
        }
    }

    private async Task RenderLoopAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            try
            {
                await Task.Delay(_renderer.ShouldUseFastRefresh()
                    ? TerminalRefreshPolicy.ActiveRefreshInterval
                    : TerminalRefreshPolicy.IdleRefreshInterval, ct).ConfigureAwait(false);
                Interlocked.Increment(ref _spinnerIndex);
                _renderer.RenderOverlay();
            }
            catch (OperationCanceledException) { return; }
            catch { return; }
        }
    }

    private async Task HandleKeyAsync(ConsoleKeyInfo key, CancellationToken ct)
    {
        if ((key.Modifiers & ConsoleModifiers.Control) != 0)
        {
            switch (key.Key)
            {
                case ConsoleKey.U:
                    _input.Clear();
                    ResetSuggestion();
                    Redraw();
                    return;
                case ConsoleKey.L:
                    _renderer.ClearScreen();
                    return;
                case ConsoleKey.D when string.IsNullOrEmpty(_input.Text()):
                    _requestShutdown();
                    return;
                case ConsoleKey.C:
                    _input.Clear();
                    ResetSuggestion();
                    _renderer.Warn("input cancelled.");
                    return;
            }
        }

        switch (key.Key)
        {
            case ConsoleKey.Enter:
                await SubmitInputAsync(ct).ConfigureAwait(false);
                return;
            case ConsoleKey.Backspace:
                _input.Backspace();
                RefreshSuggestion();
                Redraw();
                return;
            case ConsoleKey.Delete:
                _input.Delete();
                RefreshSuggestion();
                Redraw();
                return;
            case ConsoleKey.Escape:
                _input.Clear();
                ResetSuggestion();
                Redraw();
                return;
            case ConsoleKey.Tab:
                CompleteInput(showSuggestionsWhenAmbiguous: true);
                return;
            case ConsoleKey.LeftArrow:
                _input.MoveLeft();
                ResetSuggestion();
                Redraw();
                return;
            case ConsoleKey.RightArrow:
                // At the end of the line with a ghost showing, right-arrow accepts it; that is
                // the only thing right-arrow could mean there, and it saves reaching for tab.
                if (_input.AtEnd && _ghost.Length > 0)
                    CompleteInput(showSuggestionsWhenAmbiguous: false);
                else
                {
                    _input.MoveRight();
                    ResetSuggestion();
                    Redraw();
                }

                return;
            case ConsoleKey.Home:
                _input.MoveHome();
                ResetSuggestion();
                Redraw();
                return;
            case ConsoleKey.End:
                _input.MoveEnd();
                RefreshSuggestion();
                Redraw();
                return;
            case ConsoleKey.UpArrow:
                _input.PreviousHistory();
                ResetSuggestion();
                Redraw();
                return;
            case ConsoleKey.DownArrow:
                _input.NextHistory();
                ResetSuggestion();
                Redraw();
                return;
        }

        if (!char.IsControl(key.KeyChar))
        {
            _input.Append(key.KeyChar);
            RefreshSuggestion();
            Redraw();
        }
    }

    // Ghost text is only meaningful at the end of the line -- mid-line it would render after
    // text the user is still editing around and read as part of the input.
    private void RefreshSuggestion()
    {
        _lastSuggestionInput = "";
        _ghost = _input.AtEnd ? _commands.Suggest(_input.Text()) : "";
    }

    private void ResetSuggestion()
    {
        _lastSuggestionInput = "";
        _ghost = "";
    }

    private async Task SubmitInputAsync(CancellationToken ct)
    {
        string commandText = _input.Take().Trim();
        ResetSuggestion();
        if (commandText.Length == 0)
        {
            Redraw();
            return;
        }

        _input.Remember(commandText);
        _session.RecordCommand(commandText);
        _renderer.EchoCommand(commandText);

        var parsed = TerminalCommandLine.Parse(commandText);
        if (!_commands.TryGet(parsed.Verb, out TerminalCommand? command))
        {
            _renderer.RenderUnknownCommand(parsed.Verb, _commands.NearestCommands(parsed.Verb));
            return;
        }

        var context = new TerminalCommandContext(
            _renderer,
            _session,
            _commands,
            _requestShutdown,
            WatchdogStats.GetActivitySnapshot,
            MeshConnected);

        try
        {
            await command!.ExecuteAsync(context, parsed.Arguments, ct).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            _renderer.Warn("command cancelled.");
        }
        catch (Exception ex)
        {
            _renderer.Error("command failed: " + ex.GetType().Name + ": " + ex.Message);
        }
    }

    private void CompleteInput(bool showSuggestionsWhenAmbiguous)
    {
        TerminalCompletion completion = _commands.Complete(_input.Text());
        if (!string.IsNullOrEmpty(completion.Replacement))
        {
            _input.Set(completion.Replacement);
            RefreshSuggestion();
            Redraw();
            return;
        }

        if (completion.Suggestions.Count > 0 && showSuggestionsWhenAmbiguous)
            ShowCompletions(_input.Text(), completion.Suggestions, force: true);
        else
            Redraw();
    }

    private void ShowCompletions(string input, IReadOnlyList<TerminalCompletionItem> suggestions, bool force)
    {
        if (!force && string.Equals(_lastSuggestionInput, input, StringComparison.Ordinal))
        {
            Redraw();
            return;
        }

        _lastSuggestionInput = input;
        _renderer.RenderCompletions(input, suggestions);
    }

    private void Redraw()
    {
        if (!_stopped)
            _renderer.RenderOverlay();
    }

    private bool MeshConnected()
    {
        try { return _meshConnected(); }
        catch { return false; }
    }

    private static async Task AwaitNoThrowAsync(Task? task, int timeoutMs)
    {
        if (task == null) return;
        try
        {
            Task timeout = Task.Delay(timeoutMs);
            await Task.WhenAny(task, timeout).ConfigureAwait(false);
        }
        catch { /* best-effort */ }
    }

    public void Dispose()
    {
        _stopped = true;
        _cts.Cancel();
        _renderer.DetachOverlay();
        _session.Dispose();
        _cts.Dispose();
    }
}
