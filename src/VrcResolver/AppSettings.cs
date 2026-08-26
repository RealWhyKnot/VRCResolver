using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

internal sealed class AppSettings
{
    [JsonPropertyName("schema_version")]
    public int SchemaVersion { get; set; } = 1;

    [JsonPropertyName("terminal")]
    public TerminalAppSettings Terminal { get; set; } = new();

    [JsonPropertyName("relay")]
    public RelayAppSettings Relay { get; set; } = new();

    [JsonPropertyName("maintenance")]
    public MaintenanceAppSettings Maintenance { get; set; } = new();

    [JsonPropertyName("playback")]
    public PlaybackAppSettings Playback { get; set; } = new();

    public AppSettings Clone()
    {
        return new AppSettings
        {
            SchemaVersion = SchemaVersion <= 0 ? 1 : SchemaVersion,
            Terminal = (Terminal ?? new TerminalAppSettings()).Clone(),
            Relay = (Relay ?? new RelayAppSettings()).Clone(),
            Maintenance = (Maintenance ?? new MaintenanceAppSettings()).Clone(),
            Playback = (Playback ?? new PlaybackAppSettings()).Clone(),
        }.Normalize();
    }

    public AppSettings Normalize()
    {
        SchemaVersion = 1;
        Terminal ??= new TerminalAppSettings();
        Relay ??= new RelayAppSettings();
        Maintenance ??= new MaintenanceAppSettings();
        Playback ??= new PlaybackAppSettings();

        Terminal.Normalize();
        Relay.Normalize();
        Maintenance.Normalize();
        Playback.Normalize();
        return this;
    }
}

internal sealed class TerminalAppSettings
{
    [JsonPropertyName("status_line")]
    public bool StatusLine { get; set; } = true;

    [JsonPropertyName("animations")]
    public bool Animations { get; set; } = true;

    public TerminalAppSettings Clone() => new()
    {
        StatusLine = StatusLine,
        Animations = Animations,
    };

    public void Normalize()
    {
    }
}

internal sealed class RelayAppSettings
{
    public const string HttpsAuto = "auto";
    public const string HttpsOff = "off";

    [JsonPropertyName("https")]
    public string Https { get; set; } = HttpsAuto;

    public RelayAppSettings Clone() => new()
    {
        Https = Https,
    };

    public void Normalize()
    {
        Https = NormalizeHttps(Https);
    }

    public static string NormalizeHttps(string? value)
    {
        value = (value ?? "").Trim().ToLowerInvariant();
        return value switch
        {
            HttpsAuto => HttpsAuto,
            "on" => HttpsAuto,
            "enabled" => HttpsAuto,
            HttpsOff => HttpsOff,
            "disabled" => HttpsOff,
            "false" => HttpsOff,
            "no" => HttpsOff,
            _ => HttpsAuto,
        };
    }
}

internal sealed class MaintenanceAppSettings
{
    [JsonPropertyName("update_check")]
    public bool UpdateCheck { get; set; } = true;

    [JsonPropertyName("codec_auto_install")]
    public bool CodecAutoInstall { get; set; } = true;

    // Opt-in to picking up GitHub prereleases on both the startup
    // "version X is available" nudge AND the vrcresolver.Updater
    // exchange. Default false so the stable channel keeps its
    // hands-off behaviour; flipping on widens the scope of
    // "newer release" to anything tagged prerelease.
    [JsonPropertyName("include_prereleases")]
    public bool IncludePrereleases { get; set; }

    public MaintenanceAppSettings Clone() => new()
    {
        UpdateCheck = UpdateCheck,
        CodecAutoInstall = CodecAutoInstall,
        IncludePrereleases = IncludePrereleases,
    };

    public void Normalize()
    {
    }
}

internal sealed class PlaybackAppSettings
{
    // Take the best rung the source offers instead of the cap VRChat asked for. Off by
    // default: it costs bandwidth, and above 1080 most ladders are VP9/AV01, which only
    // plays if this machine has a decoder for them. There is no way to pick a quality from
    // inside VRChat, so this is the one place the choice can be made.
    [JsonPropertyName("high_quality")]
    public bool HighQuality { get; set; }

    public PlaybackAppSettings Clone() => new()
    {
        HighQuality = HighQuality,
    };

    public void Normalize()
    {
    }
}

internal sealed class AppSettingsStore
{
    private const long MaxSettingsBytes = 128 * 1024;
    private readonly object _lock = new();
    private readonly string _path;
    private AppSettings? _cached;

    public static AppSettingsStore Shared { get; } =
        new(System.IO.Path.Combine(AppPaths.StateRoot(), "settings.json"));

    public AppSettingsStore(string path)
    {
        _path = path ?? throw new ArgumentNullException(nameof(path));
    }

    public string FilePath => _path;

    public AppSettings Snapshot()
    {
        lock (_lock)
        {
            _cached ??= LoadNoThrow();
            return _cached.Clone();
        }
    }

    public AppSettings Update(Action<AppSettings> mutate)
    {
        if (mutate == null) throw new ArgumentNullException(nameof(mutate));
        lock (_lock)
        {
            _cached ??= LoadNoThrow();
            var next = _cached.Clone();
            mutate(next);
            next.Normalize();
            SaveNoThrow(next);
            _cached = next;
            return next.Clone();
        }
    }

    private AppSettings LoadNoThrow()
    {
        try
        {
            if (!File.Exists(_path))
                return new AppSettings();

            var info = new FileInfo(_path);
            if (info.Length > MaxSettingsBytes)
            {
                QuarantineInvalidFile("too-large");
                return new AppSettings();
            }

            using var stream = File.OpenRead(_path);
            return (JsonSerializer.Deserialize(stream, MeshJsonContext.Default.AppSettings)
                ?? new AppSettings()).Normalize();
        }
        catch
        {
            QuarantineInvalidFile("invalid");
            return new AppSettings();
        }
    }

    private void SaveNoThrow(AppSettings settings)
    {
        try
        {
            string? dir = System.IO.Path.GetDirectoryName(_path);
            if (!string.IsNullOrEmpty(dir))
                Directory.CreateDirectory(dir);

            string tmp = _path + ".new";
            using (var stream = File.Create(tmp))
                JsonSerializer.Serialize(stream, settings.Normalize(), MeshJsonContext.Default.AppSettings);
            File.Move(tmp, _path, overwrite: true);
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Terminal, "settings save failed: " + ex.Message);
        }
    }

    private void QuarantineInvalidFile(string reason)
    {
        try
        {
            if (!File.Exists(_path)) return;
            string dst = _path + "." + reason + "." + DateTime.UtcNow.ToString("yyyyMMddHHmmss") + ".bak";
            File.Move(_path, dst, overwrite: true);
            ConsoleUx.Warn(LogComponent.Terminal, "ignored invalid settings file; moved to " + System.IO.Path.GetFileName(dst));
        }
        catch
        {
            // If quarantine fails, defaults are still safer than trusting bad JSON.
        }
    }
}
