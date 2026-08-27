using System.Diagnostics;
using System.Runtime.Versioning;
using System.Text.Json;
using System.Text.Json.Serialization;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class CodecInstaller
{
    private static readonly TimeSpan PerCodecTimeout = TimeSpan.FromSeconds(60);
    internal static readonly TimeSpan TransientRetryWindow = TimeSpan.FromDays(1);
    internal static readonly TimeSpan PermanentRetryWindow = TimeSpan.FromDays(7);

    internal sealed record Codec(string Name, string PackageName, string ProbeCodec, string[] StoreIds);

    internal static readonly Codec[] Required =
    {
        new("AV1 Video Extension", "Microsoft.AV1VideoExtension", "av1",
            new[] { "9MVZQVXJBQ9V" }),
        new("HEVC Video Extensions", "Microsoft.HEVCVideoExtension", "h265",
            new[] { "9NMZLZ57R3T7", "9N4WGH0Z6VHQ" }),
        new("VP9 Video Extensions", "Microsoft.VP9VideoExtensions", "vp9",
            new[] { "9N4D0MSV0403" }),
    };

    public static void StartBackgroundCheck()
    {
        if (!AppSettingsStore.Shared.Snapshot().Maintenance.CodecAutoInstall)
        {
            Logger.WriteFileOnly("[codec] auto-install disabled by settings");
            return;
        }

        _ = Task.Run(RunAsync);
    }

    private static async Task RunAsync()
    {
        try
        {
            var statePath = StatePath();
            var state = LoadState(statePath);
            var core = new Core(
                mfProbe: codec => CodecCapabilityProbe.ProbeDecoder(codec) == true,
                appxProbe: IsAppxInstalledAsync,
                wingetInstall: WingetInstallAsync,
                utcNow: () => DateTime.UtcNow);

            bool dirty = await core.RunAsync(Required, state, outcome =>
            {
                if (outcome.Installed)
                {
                    ConsoleUx.Success(LogComponent.Codec, outcome.Codec.Name + " ready ("
                        + outcome.Detail + ").");
                    CodecCapabilityProbe.Refresh();
                }
                else
                {
                    ConsoleUx.Warn(LogComponent.Codec, outcome.Codec.Name + " install failed ("
                        + outcome.Detail + "); will retry "
                        + (IsTransientFailure(outcome.Detail) ? "tomorrow" : "next week") + ".");
                }
            }).ConfigureAwait(false);

            if (dirty) SaveState(statePath, state);
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Codec, "background check failed: " + ex.Message);
        }
    }

    internal sealed record CodecOutcome(Codec Codec, bool Installed, string Detail);

    internal const string FailWingetMissing = "winget_missing";
    internal const string FailNotEntitled = "not_entitled";
    internal const string FailNotFound = "not_found";
    internal const string FailSourceUnavailable = "msstore_source_unavailable";
    internal const string FailTimeout = "timeout";
    internal const string FailUnverifiedInstall = "install_unverified";
    internal const string FailUnknown = "unknown";

    internal static bool IsTransientFailure(string failureClass) => failureClass
        is FailTimeout or FailUnverifiedInstall or FailUnknown;

    internal sealed record WingetResult(bool Launched, bool TimedOut, int ExitCode, string Output);

    internal static string ClassifyWingetFailure(WingetResult result)
    {
        if (!result.Launched) return FailWingetMissing;
        if (result.TimedOut) return FailTimeout;
        string body = result.Output.ToLowerInvariant();
        if (body.Contains("no package found")) return FailNotFound;
        if (body.Contains("purchase") || body.Contains("not owned") || body.Contains("entitlement"))
            return FailNotEntitled;
        if (body.Contains("source") && (body.Contains("disabled") || body.Contains("agreement") || body.Contains("blocked")))
            return FailSourceUnavailable;
        return FailUnknown;
    }

    internal sealed class Core
    {
        private readonly Func<string, bool> _mfProbe;
        private readonly Func<string, Task<bool>> _appxProbe;
        private readonly Func<string, Task<WingetResult>> _wingetInstall;
        private readonly Func<DateTime> _utcNow;

        public Core(
            Func<string, bool> mfProbe,
            Func<string, Task<bool>> appxProbe,
            Func<string, Task<WingetResult>> wingetInstall,
            Func<DateTime> utcNow)
        {
            _mfProbe = mfProbe;
            _appxProbe = appxProbe;
            _wingetInstall = wingetInstall;
            _utcNow = utcNow;
        }

        public async Task<bool> RunAsync(
            IReadOnlyList<Codec> codecs, CodecState state, Action<CodecOutcome>? report = null)
        {
            bool dirty = false;
            foreach (var codec in codecs)
            {
                if (ShouldSkip(state, codec.StoreIds[0])) continue;

                var outcome = await ResolveOneAsync(codec).ConfigureAwait(false);
                state.Codecs[codec.StoreIds[0]] = new CodecEntry
                {
                    Status = outcome.Installed ? "installed" : "failed",
                    FailureClass = outcome.Installed ? null : outcome.Detail,
                    LastAttemptUtc = _utcNow(),
                    PackageName = codec.PackageName,
                };
                dirty = true;
                report?.Invoke(outcome);
            }
            return dirty;
        }

        private async Task<CodecOutcome> ResolveOneAsync(Codec codec)
        {
            if (_mfProbe(codec.ProbeCodec))
                return new CodecOutcome(codec, Installed: true, "mf");
            if (await _appxProbe(codec.PackageName).ConfigureAwait(false))
                return new CodecOutcome(codec, Installed: true, "appx");

            string lastClass = FailUnknown;
            foreach (var storeId in codec.StoreIds)
            {
                var result = await _wingetInstall(storeId).ConfigureAwait(false);
                if (result.Launched && !result.TimedOut && result.ExitCode == 0)
                {
                    if (_mfProbe(codec.ProbeCodec))
                        return new CodecOutcome(codec, Installed: true, "winget+mf");
                    if (await _appxProbe(codec.PackageName).ConfigureAwait(false))
                        return new CodecOutcome(codec, Installed: true, "winget+appx");
                    lastClass = FailUnverifiedInstall;
                    continue;
                }
                lastClass = ClassifyWingetFailure(result);
                if (lastClass == FailWingetMissing) break;
            }
            return new CodecOutcome(codec, Installed: false, lastClass);
        }

        internal bool ShouldSkip(CodecState state, string key)
        {
            if (!state.Codecs.TryGetValue(key, out var entry)) return false;
            if (entry.Status == "installed") return true;
            if (entry.Status != "failed") return false;
            var window = IsTransientFailure(entry.FailureClass ?? FailUnknown)
                ? TransientRetryWindow
                : PermanentRetryWindow;
            return _utcNow() - entry.LastAttemptUtc < window;
        }
    }

    private static async Task<bool> IsAppxInstalledAsync(string packageName)
    {
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "powershell.exe",
                ArgumentList =
                {
                    "-NoProfile", "-ExecutionPolicy", "Bypass",
                    "-Command", "Get-AppxPackage -Name '" + packageName + "' | Select-Object -ExpandProperty Name",
                },
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
            };
            using var proc = Process.Start(psi);
            if (proc == null) return false;
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            try { await proc.WaitForExitAsync(cts.Token).ConfigureAwait(false); }
            catch (OperationCanceledException) { try { proc.Kill(true); } catch { } return false; }
            string output = await proc.StandardOutput.ReadToEndAsync().ConfigureAwait(false);
            return !string.IsNullOrWhiteSpace(output);
        }
        catch
        {
            return false;
        }
    }

    private static async Task<WingetResult> WingetInstallAsync(string storeId)
    {
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "winget",
                ArgumentList =
                {
                    "install", "--id", storeId, "--source", "msstore",
                    "--accept-package-agreements", "--accept-source-agreements",
                    "--silent",
                },
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
            };
            using var proc = Process.Start(psi);
            if (proc == null) return new WingetResult(false, false, -1, "");
            var stdout = proc.StandardOutput.ReadToEndAsync();
            var stderr = proc.StandardError.ReadToEndAsync();
            using var cts = new CancellationTokenSource(PerCodecTimeout);
            try { await proc.WaitForExitAsync(cts.Token).ConfigureAwait(false); }
            catch (OperationCanceledException)
            {
                try { proc.Kill(true); } catch { }
                return new WingetResult(true, true, -1, "");
            }
            string output = (await stdout.ConfigureAwait(false)) + "\n" + (await stderr.ConfigureAwait(false));
            if (output.Length > 8 * 1024) output = output[..(8 * 1024)];
            Logger.WriteFileOnly("[codec] winget " + storeId + " exit=" + proc.ExitCode
                + " out=" + LogUtil.SanitizeForConsole(output, 400));
            return new WingetResult(true, false, proc.ExitCode, output);
        }
        catch (System.ComponentModel.Win32Exception)
        {
            return new WingetResult(false, false, -1, "");
        }
        catch (Exception ex)
        {
            return new WingetResult(true, false, -1, ex.GetType().Name);
        }
    }

    private static string StatePath()
    {
        string dir = AppPaths.StateRoot();
        Directory.CreateDirectory(dir);
        return Path.Combine(dir, "codec-state.json");
    }

    internal static CodecState LoadState(string path)
    {
        try
        {
            if (!File.Exists(path)) return new CodecState();
            string json = File.ReadAllText(path);
            return JsonSerializer.Deserialize(json, MeshJsonContext.Default.CodecState) ?? new CodecState();
        }
        catch
        {
            return new CodecState();
        }
    }

    internal static void SaveState(string path, CodecState state)
    {
        try
        {
            string tmp = path + ".new";
            File.WriteAllText(tmp, JsonSerializer.Serialize(state, MeshJsonContext.Default.CodecState));
            File.Move(tmp, path, overwrite: true);
        }
        catch { }
    }

    internal sealed class CodecState
    {
        [JsonPropertyName("codecs")]
        public Dictionary<string, CodecEntry> Codecs { get; set; } = new();
    }

    internal sealed class CodecEntry
    {
        [JsonPropertyName("status")] public string Status { get; set; } = "";
        [JsonPropertyName("failure_class")] public string? FailureClass { get; set; }
        [JsonPropertyName("last_attempt_utc")] public DateTime LastAttemptUtc { get; set; }
        [JsonPropertyName("package_family_name")] public string PackageName { get; set; } = "";
    }
}
