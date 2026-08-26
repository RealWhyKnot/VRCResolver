namespace VrcResolver;

internal static class AppSettingsRegistry
{
    private static readonly AppSettings s_defaults = new();

    public static IReadOnlyList<AppSettingDefinition> All { get; } =
    [
        new AppSettingDefinition(
            "status-line",
            "Show the live status line at the prompt.",
            ["on", "off"],
            static s => FormatBool(s.Terminal.StatusLine),
            static (AppSettings s, string value, out string error) =>
            {
                if (!TryParseBool(value, out bool parsed, out error)) return false;
                s.Terminal.StatusLine = parsed;
                return true;
            },
            static s => s.Terminal.StatusLine = s_defaults.Terminal.StatusLine,
            completionValues: ["on", "off"],
            aliases: ["status", "terminal.status-line"]),

        new AppSettingDefinition(
            "animations",
            "Animate the active video traffic indicators.",
            ["on", "off"],
            static s => FormatBool(s.Terminal.Animations),
            static (AppSettings s, string value, out string error) =>
            {
                if (!TryParseBool(value, out bool parsed, out error)) return false;
                s.Terminal.Animations = parsed;
                return true;
            },
            static s => s.Terminal.Animations = s_defaults.Terminal.Animations,
            completionValues: ["on", "off"],
            aliases: ["terminal.animations"]),

        new AppSettingDefinition(
            "secure-local-video",
            "Use secure local video links when Windows allows it.",
            ["on", "off"],
            static s => s.Relay.Https == RelayAppSettings.HttpsOff ? "off" : "on",
            static (AppSettings s, string value, out string error) =>
            {
                if (!TryParseBool(value, out bool parsed, out error)) return false;
                s.Relay.Https = parsed ? RelayAppSettings.HttpsAuto : RelayAppSettings.HttpsOff;
                return true;
            },
            static s => s.Relay.Https = s_defaults.Relay.Https,
            restartRequired: true,
            completionValues: ["on", "off"],
            aliases: ["local-https", "relay.https"]),

        new AppSettingDefinition(
            "update-check",
            "Check for new VRCResolver versions at startup.",
            ["on", "off"],
            static s => FormatBool(s.Maintenance.UpdateCheck),
            static (AppSettings s, string value, out string error) =>
            {
                if (!TryParseBool(value, out bool parsed, out error)) return false;
                s.Maintenance.UpdateCheck = parsed;
                return true;
            },
            static s => s.Maintenance.UpdateCheck = s_defaults.Maintenance.UpdateCheck,
            restartRequired: true,
            completionValues: ["on", "off"],
            aliases: ["updates", "maintenance.update-check"]),

        new AppSettingDefinition(
            "include-prereleases",
            "Offer prereleases when checking for new versions.",
            ["on", "off"],
            static s => FormatBool(s.Maintenance.IncludePrereleases),
            static (AppSettings s, string value, out string error) =>
            {
                if (!TryParseBool(value, out bool parsed, out error)) return false;
                s.Maintenance.IncludePrereleases = parsed;
                return true;
            },
            static s => s.Maintenance.IncludePrereleases = s_defaults.Maintenance.IncludePrereleases,
            restartRequired: true,
            completionValues: ["on", "off"],
            aliases: ["prereleases", "prerelease", "beta", "maintenance.include-prereleases"]),

        new AppSettingDefinition(
            "video-support-updates",
            "Keep playback support files current.",
            ["on", "off"],
            static s => FormatBool(s.Maintenance.CodecAutoInstall),
            static (AppSettings s, string value, out string error) =>
            {
                if (!TryParseBool(value, out bool parsed, out error)) return false;
                s.Maintenance.CodecAutoInstall = parsed;
                return true;
            },
            static s =>
            {
                s.Maintenance.CodecAutoInstall = s_defaults.Maintenance.CodecAutoInstall;
            },
            restartRequired: true,
            completionValues: ["on", "off"],
            aliases: ["video-updates", "codec-install", "maintenance.codec-install"]),

        new AppSettingDefinition(
            "high-quality",
            "Play the best rung a video offers instead of the quality VRChat asked for. Uses more bandwidth, and above 1080p most sources need a VP9 or AV1 decoder.",
            ["on", "off"],
            static s => FormatBool(s.Playback.HighQuality),
            static (AppSettings s, string value, out string error) =>
            {
                if (!TryParseBool(value, out bool parsed, out error)) return false;
                s.Playback.HighQuality = parsed;
                return true;
            },
            static s =>
            {
                s.Playback.HighQuality = s_defaults.Playback.HighQuality;
            },
            restartRequired: false,
            completionValues: ["on", "off"],
            aliases: ["hq", "quality", "playback.high-quality"]),
    ];

    public static bool TryFind(string key, out AppSettingDefinition? setting)
    {
        key = (key ?? "").Trim();
        setting = All.FirstOrDefault(s =>
            string.Equals(s.Key, key, StringComparison.OrdinalIgnoreCase)
            || s.Aliases.Any(a => string.Equals(a, key, StringComparison.OrdinalIgnoreCase)));
        return setting != null;
    }

    private static bool TryParseBool(string value, out bool parsed, out string error)
    {
        switch ((value ?? "").Trim().ToLowerInvariant())
        {
            case "on":
            case "true":
            case "yes":
            case "1":
            case "enabled":
                parsed = true;
                error = "";
                return true;
            case "off":
            case "false":
            case "no":
            case "0":
            case "disabled":
                parsed = false;
                error = "";
                return true;
            default:
                parsed = false;
                error = "expected on or off";
                return false;
        }
    }

    private static string FormatBool(bool value) => value ? "on" : "off";
}

internal sealed class AppSettingDefinition
{
    private readonly Func<AppSettings, string> _get;
    private readonly TrySetSetting _set;
    private readonly Action<AppSettings> _reset;

    public AppSettingDefinition(
        string key,
        string description,
        IReadOnlyList<string> choices,
        Func<AppSettings, string> get,
        TrySetSetting set,
        Action<AppSettings> reset,
        bool restartRequired = false,
        IReadOnlyList<string>? completionValues = null,
        IReadOnlyList<string>? aliases = null)
    {
        Key = key;
        Description = description;
        Choices = choices;
        _get = get;
        _set = set;
        _reset = reset;
        RestartRequired = restartRequired;
        CompletionValues = completionValues ?? choices;
        Aliases = aliases ?? Array.Empty<string>();
    }

    public string Key { get; }
    public string Description { get; }
    public IReadOnlyList<string> Choices { get; }
    public IReadOnlyList<string> CompletionValues { get; }
    public bool RestartRequired { get; }
    public IReadOnlyList<string> Aliases { get; }

    public string Get(AppSettings settings) => _get(settings);

    public bool TrySet(AppSettings settings, string value, out string error) => _set(settings, value, out error);

    public void Reset(AppSettings settings) => _reset(settings);
}

internal delegate bool TrySetSetting(AppSettings settings, string value, out string error);
