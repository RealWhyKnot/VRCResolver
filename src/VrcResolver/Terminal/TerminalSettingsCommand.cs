namespace VrcResolver;

internal static class TerminalSettingsCommand
{
    public static Task ExecuteAsync(TerminalCommandContext ctx, string args, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        var parts = Split(args);
        if (parts.Count == 0)
        {
            ctx.Renderer.RenderSettings(AppSettingsRegistry.All, AppSettingsStore.Shared.Snapshot());
            return Task.CompletedTask;
        }

        string action = parts[0].ToLowerInvariant();
        switch (action)
        {
            case "help":
                ctx.Renderer.RenderSettingsHelp();
                return Task.CompletedTask;

            case "get":
                if (parts.Count != 2)
                {
                    ctx.Renderer.Warn("usage: settings get <name>");
                    return Task.CompletedTask;
                }
                RenderOne(ctx, parts[1]);
                return Task.CompletedTask;

            case "set":
                if (parts.Count < 3)
                {
                    ctx.Renderer.Warn("usage: settings set <name> <value>");
                    return Task.CompletedTask;
                }
                SetOne(ctx, parts[1], string.Join(" ", parts.Skip(2)));
                return Task.CompletedTask;

            case "reset":
                if (parts.Count != 2)
                {
                    ctx.Renderer.Warn("usage: settings reset <name|all>");
                    return Task.CompletedTask;
                }
                Reset(ctx, parts[1]);
                return Task.CompletedTask;

            default:
                if (parts.Count >= 2)
                {
                    SetOne(ctx, parts[0], string.Join(" ", parts.Skip(1)));
                    return Task.CompletedTask;
                }

                RenderOne(ctx, parts[0]);
                return Task.CompletedTask;
        }
    }

    private static void RenderOne(TerminalCommandContext ctx, string key)
    {
        if (!AppSettingsRegistry.TryFind(key, out AppSettingDefinition? setting))
        {
            ctx.Renderer.Warn("unknown setting: " + key + " (use settings to list names)");
            return;
        }

        ctx.Renderer.RenderSetting(setting!, AppSettingsStore.Shared.Snapshot());
    }

    private static void SetOne(TerminalCommandContext ctx, string key, string value)
    {
        if (!AppSettingsRegistry.TryFind(key, out AppSettingDefinition? setting))
        {
            ctx.Renderer.Warn("unknown setting: " + key + " (use settings to list names)");
            return;
        }

        AppSettings probe = AppSettingsStore.Shared.Snapshot();
        if (!setting!.TrySet(probe, value, out string error))
        {
            ctx.Renderer.Warn("invalid value for " + key + ": " + error);
            return;
        }

        AppSettings next = AppSettingsStore.Shared.Update(settings => setting!.TrySet(settings, value, out _));
        string suffix = setting!.RestartRequired ? " (takes effect next launch)" : "";
        ctx.Renderer.Success("settings: " + key + " = " + setting.Get(next) + suffix);
    }

    private static void Reset(TerminalCommandContext ctx, string key)
    {
        if (string.Equals(key, "all", StringComparison.OrdinalIgnoreCase))
        {
            AppSettingsStore.Shared.Update(settings =>
            {
                foreach (var setting in AppSettingsRegistry.All)
                    setting.Reset(settings);
            });
            ctx.Renderer.Success("settings: all values reset to defaults.");
            return;
        }

        if (!AppSettingsRegistry.TryFind(key, out AppSettingDefinition? setting))
        {
            ctx.Renderer.Warn("unknown setting: " + key + " (use settings to list names)");
            return;
        }

        AppSettings next = AppSettingsStore.Shared.Update(settings => setting!.Reset(settings));
        ctx.Renderer.Success("settings: " + key + " reset to " + setting!.Get(next));
    }

    private static IReadOnlyList<string> Split(string args)
    {
        return (args ?? "")
            .Split(' ', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
    }
}
