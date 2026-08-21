namespace VrcResolver;

internal sealed class TerminalCommandRegistry
{
    private readonly Dictionary<string, TerminalCommand> _commands = new(StringComparer.OrdinalIgnoreCase);
    private readonly List<TerminalCommand> _primaryCommands = new();

    public IReadOnlyList<TerminalCommand> All => _primaryCommands;

    public static TerminalCommandRegistry CreateDefault()
    {
        var registry = new TerminalCommandRegistry();

        registry.Add(
            "help",
            "Show available terminal commands.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.RenderHelp(ctx.Commands.All);
                return Task.CompletedTask;
            },
            "?");

        registry.Add(
            "settings",
            "List or change persisted watchdog settings.",
            TerminalSettingsCommand.ExecuteAsync,
            "config");

        registry.Add(
            "status",
            "Show what the proxy is doing right now.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.RenderStatus(ctx.GetSnapshot(), ctx.MeshConnected());
                return Task.CompletedTask;
            },
            "stats",
            "activity",
            "dashboard");

        registry.Add(
            "history",
            "Show recent terminal commands.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.RenderHistory(ctx.Session.RecentCommands);
                return Task.CompletedTask;
            });

        registry.Add(
            "permissions",
            "Show what this terminal is allowed to do.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.Info("permissions: local watchdog commands only; shell execution is not exposed.");
                return Task.CompletedTask;
            });

        registry.Add(
            "tools",
            "Show managed watchdog subsystems.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.RenderTools();
                return Task.CompletedTask;
            });

        registry.Add(
            "diagnostics",
            "Show useful support paths and session files.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.RenderDiagnostics(ctx.Session);
                return Task.CompletedTask;
            },
            "diag",
            "paths");

        registry.Add(
            "update",
            "Check for and install the latest VRCResolver release.",
            UpdateCommand.ExecuteAsync,
            "upgrade");

        registry.Add(
            "clear",
            "Clear the terminal view.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.ClearScreen();
                return Task.CompletedTask;
            },
            "cls");

        registry.Add(
            "quit",
            "Shut down VRCResolver cleanly.",
            static (ctx, _, _) =>
            {
                ctx.Renderer.Info("quit requested; shutting down.");
                ctx.RequestShutdown();
                return Task.CompletedTask;
            },
            "exit");

        return registry;
    }

    public void Add(
        string name,
        string description,
        Func<TerminalCommandContext, string, CancellationToken, Task> handler,
        params string[] aliases)
    {
        if (string.IsNullOrWhiteSpace(name))
            throw new ArgumentException("Command name is required.", nameof(name));
        if (handler == null)
            throw new ArgumentNullException(nameof(handler));

        var command = new TerminalCommand(
            TerminalCommandLine.NormalizeVerb(name),
            description ?? "",
            handler,
            aliases.Select(TerminalCommandLine.NormalizeVerb).Where(a => a.Length > 0).ToArray());

        _primaryCommands.Add(command);
        _commands[command.Name] = command;
        foreach (string alias in command.Aliases)
            _commands[alias] = command;
    }

    public bool TryGet(string verb, out TerminalCommand? command)
    {
        return _commands.TryGetValue(TerminalCommandLine.NormalizeVerb(verb), out command);
    }

    public TerminalCompletion Complete(string input)
    {
        input ??= "";
        string trimmed = input.TrimStart();
        bool slash = trimmed.StartsWith("/", StringComparison.Ordinal);
        string body = slash ? trimmed.Substring(1) : trimmed;
        bool endsWithSpace = body.Length > 0 && char.IsWhiteSpace(body[^1]);
        string[] tokens = body.Split(new[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries);
        if (!endsWithSpace && tokens.Length <= 1)
            return CompleteCommand(slash, tokens.Length == 0 ? "" : tokens[0]);

        if (tokens.Length == 0)
            return CompleteCommand(slash, "");

        string verb = TerminalCommandLine.NormalizeVerb(tokens[0]);
        if (!TryGet(verb, out TerminalCommand? command) || command == null)
            return TerminalCompletion.Empty;
        if (!string.Equals(command.Name, "settings", StringComparison.OrdinalIgnoreCase))
            return TerminalCompletion.Empty;

        return CompleteSettings(slash, tokens, endsWithSpace);
    }

    private TerminalCompletion CompleteCommand(bool slash, string token)
    {
        string prefix = TerminalCommandLine.NormalizeVerb(token);
        var matches = _primaryCommands
            .Where(c => c.Name.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)
                || c.Aliases.Any(a => a.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)))
            .OrderBy(c => c.Name, StringComparer.OrdinalIgnoreCase)
            .ToArray();

        if (prefix.Length == 0)
            return new TerminalCompletion("", CommandItems(matches, slash));

        if (matches.Length == 1)
            return new TerminalCompletion((slash ? "/" : "") + matches[0].Name + " ", Array.Empty<TerminalCompletionItem>());

        return new TerminalCompletion("", CommandItems(matches, slash));
    }

    private static TerminalCompletion CompleteSettings(bool slash, string[] tokens, bool endsWithSpace)
    {
        var args = tokens.Skip(1).ToList();
        if (endsWithSpace)
            args.Add("");

        if (args.Count == 0)
            args.Add("");

        string commandPrefix = slash ? "/settings" : "settings";
        string current = args[^1];
        if (args.Count == 1)
        {
            var items = SettingsActionItems(current)
                .Concat(SettingItems(current))
                .ToArray();
            if (items.Length == 1)
            {
                string replacement = commandPrefix + " " + items[0].Text
                    + (IsSettingName(items[0].Text) ? " " : " ");
                return new TerminalCompletion(replacement, Array.Empty<TerminalCompletionItem>());
            }

            return new TerminalCompletion("", items);
        }

        string action = args[0].ToLowerInvariant();
        if (action is "get" or "reset")
        {
            var items = SettingItems(current, includeAll: action == "reset").ToArray();
            if (items.Length == 1)
                return new TerminalCompletion(commandPrefix + " " + action + " " + items[0].Text + " ", Array.Empty<TerminalCompletionItem>());
            return new TerminalCompletion("", items);
        }

        if (action == "set")
        {
            if (args.Count == 2)
            {
                var items = SettingItems(current).ToArray();
                if (items.Length == 1)
                    return new TerminalCompletion(commandPrefix + " set " + items[0].Text + " ", Array.Empty<TerminalCompletionItem>());
                return new TerminalCompletion("", items);
            }

            return CompleteSettingValue(commandPrefix + " set ", args[1], current);
        }

        return CompleteSettingValue(commandPrefix + " ", args[0], current);
    }

    private static TerminalCompletion CompleteSettingValue(string prefix, string settingName, string current)
    {
        if (!AppSettingsRegistry.TryFind(settingName, out AppSettingDefinition? setting) || setting == null)
            return TerminalCompletion.Empty;

        var items = setting.CompletionValues
            .Where(v => v.StartsWith(current ?? "", StringComparison.OrdinalIgnoreCase))
            .Select(v => new TerminalCompletionItem(v, setting.Description))
            .ToArray();
        if (items.Length == 1)
            return new TerminalCompletion(prefix + setting.Key + " " + items[0].Text, Array.Empty<TerminalCompletionItem>());

        return new TerminalCompletion("", items);
    }

    private static IEnumerable<TerminalCompletionItem> SettingsActionItems(string prefix)
    {
        var actions = new[]
        {
            new TerminalCompletionItem("get", "Show one setting."),
            new TerminalCompletionItem("set", "Change a setting."),
            new TerminalCompletionItem("reset", "Reset a setting."),
            new TerminalCompletionItem("help", "Show settings help."),
        };
        return actions.Where(i => i.Text.StartsWith(prefix ?? "", StringComparison.OrdinalIgnoreCase));
    }

    private static IEnumerable<TerminalCompletionItem> SettingItems(string prefix, bool includeAll = false)
    {
        if (includeAll && "all".StartsWith(prefix ?? "", StringComparison.OrdinalIgnoreCase))
            yield return new TerminalCompletionItem("all", "Reset every setting.");

        foreach (var setting in AppSettingsRegistry.All)
        {
            if (setting.Key.StartsWith(prefix ?? "", StringComparison.OrdinalIgnoreCase)
                || setting.Aliases.Any(a => a.StartsWith(prefix ?? "", StringComparison.OrdinalIgnoreCase)))
            {
                yield return new TerminalCompletionItem(setting.Key, setting.Description);
            }
        }
    }

    private static bool IsSettingName(string value)
        => AppSettingsRegistry.TryFind(value, out _);

    private static IReadOnlyList<TerminalCompletionItem> CommandItems(IEnumerable<TerminalCommand> commands, bool slash)
    {
        return commands
            .Select(c => new TerminalCompletionItem((slash ? "/" : "") + c.Name, c.Description))
            .ToArray();
    }
}

internal sealed class TerminalCommand
{
    private readonly Func<TerminalCommandContext, string, CancellationToken, Task> _handler;

    public TerminalCommand(
        string name,
        string description,
        Func<TerminalCommandContext, string, CancellationToken, Task> handler,
        IReadOnlyList<string> aliases)
    {
        Name = name;
        Description = description;
        _handler = handler;
        Aliases = aliases;
    }

    public string Name { get; }
    public string Description { get; }
    public IReadOnlyList<string> Aliases { get; }

    public Task ExecuteAsync(TerminalCommandContext context, string arguments, CancellationToken ct)
    {
        return _handler(context, arguments, ct);
    }
}

internal sealed class TerminalCommandContext
{
    public TerminalCommandContext(
        TerminalRenderer renderer,
        TerminalSessionStore session,
        TerminalCommandRegistry commands,
        Action requestShutdown,
        Func<WatchdogActivitySnapshot> getSnapshot,
        Func<bool> meshConnected)
    {
        Renderer = renderer;
        Session = session;
        Commands = commands;
        RequestShutdown = requestShutdown;
        GetSnapshot = getSnapshot;
        MeshConnected = meshConnected;
    }

    public TerminalRenderer Renderer { get; }
    public TerminalSessionStore Session { get; }
    public TerminalCommandRegistry Commands { get; }
    public Action RequestShutdown { get; }
    public Func<WatchdogActivitySnapshot> GetSnapshot { get; }
    public Func<bool> MeshConnected { get; }
}

internal readonly record struct TerminalCompletion(
    string Replacement,
    IReadOnlyList<TerminalCompletionItem> Suggestions)
{
    public static TerminalCompletion Empty { get; } =
        new TerminalCompletion("", Array.Empty<TerminalCompletionItem>());
}

internal readonly record struct TerminalCompletionItem(string Text, string Description);
