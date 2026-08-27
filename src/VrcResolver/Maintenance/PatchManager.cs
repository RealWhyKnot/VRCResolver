using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal sealed class PatchManager : IDisposable
{
    private const int TickDelayMs = 3000;
    private const int MinReapplyGapSec = 3;

    private readonly string _patchedYtDlpPath;
    private readonly string _knownHashesPath;
    private readonly string _cleanExitFlagPath;
    private readonly string _haltFlagPath;
    private readonly CancellationTokenSource _cts = new();
    private readonly string? _vrcToolsDir;
    private Task? _loop;
    private DateTime _lastPatchTime = DateTime.MinValue;
    private bool _halted;
    private int _started;
    private int _stopping;

    private string? _classifyCachePath;
    private long _classifyCacheSize;
    private DateTime _classifyCacheMtime;
    private WrapperKind _classifyCacheKind;

    private TickOutcome _lastTickOutcome = TickOutcome.None;

    private enum TickOutcome { None, Match, Locked, Reapplied, ReapplyFailed, BackupCreated, InitialStaged, Waiting, UnknownTarget, BackupLost }

    public string? VrcToolsDir => _vrcToolsDir;
    public bool Halted => _halted;

    public static void LogVrcProcessState()
    {
        try
        {
            var procs = System.Diagnostics.Process.GetProcessesByName("VRChat");
            if (procs.Length == 0)
            {
                ConsoleUx.Write(LogComponent.Patch, "VRChat not detected -- patch will apply immediately.");
                return;
            }
            var primary = procs[0];
            DateTime started = DateTime.MinValue;
            foreach (var p in procs)
            {
                try
                {
                    if (started == DateTime.MinValue || p.StartTime < started)
                    {
                        started = p.StartTime;
                        primary = p;
                    }
                }
                catch { }
            }
            string startedStr;
            try { startedStr = primary.StartTime.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ"); }
            catch { startedStr = "<unknown>"; }
            ConsoleUx.Write(
                LogComponent.Patch,
                "VRChat is currently running (PID " + primary.Id +
                ", started " + startedStr + ") -- patch will apply when yt-dlp.exe isn't actively in use.");
            foreach (var p in procs) try { p.Dispose(); } catch { }
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Patch, "could not enumerate VRChat processes: " + ex.GetType().Name + ": " + ex.Message);
        }
    }

    public PatchManager(string installDir)
    {
        _patchedYtDlpPath = Path.Combine(installDir, "tools", "yt-dlp.exe");
        _knownHashesPath = Path.Combine(installDir, "data", "wrapper_hashes.txt");

        string stateDir = AppPaths.StateRoot();
        Directory.CreateDirectory(stateDir);
        _cleanExitFlagPath = Path.Combine(stateDir, "clean_exit.flag");
        _haltFlagPath = Path.Combine(stateDir, "halt.flag");

        _vrcToolsDir = VrcPathLocator.Find();
    }

    private WrapperKind ClassifyTarget(string path)
    {
        try
        {
            var info = new FileInfo(path);
            if (info.Exists
                && _classifyCachePath == path
                && _classifyCacheSize == info.Length
                && _classifyCacheMtime == info.LastWriteTimeUtc)
            {
                return _classifyCacheKind;
            }

            WrapperKind kind = WrapperIdentity.Classify(path, _knownHashesPath);
            if (info.Exists)
            {
                _classifyCachePath = path;
                _classifyCacheSize = info.Length;
                _classifyCacheMtime = info.LastWriteTimeUtc;
                _classifyCacheKind = kind;
            }
            return kind;
        }
        catch
        {
            return WrapperKind.Unknown;
        }
    }

    public void RecoverFromUncleanShutdown()
    {
        ToolsDirSweeper.Sweep(_vrcToolsDir);

        bool cleanLastTime = File.Exists(_cleanExitFlagPath);
        if (cleanLastTime)
        {
            try { File.Delete(_cleanExitFlagPath); } catch { }
            return;
        }

        if (string.IsNullOrEmpty(_vrcToolsDir) || !Directory.Exists(_vrcToolsDir)) return;

        string targetPath = Path.Combine(_vrcToolsDir, "yt-dlp.exe");
        string backupPath = Path.Combine(_vrcToolsDir, "yt-dlp-og.exe");

        if (File.Exists(backupPath))
        {
            ConsoleUx.Warn(LogComponent.Patch, "Recovery: previous run exited uncleanly -- restoring VRChat's yt-dlp from yt-dlp-og.exe.");
            RestoreYtDlpInTools(_vrcToolsDir);
            return;
        }

        if (File.Exists(targetPath) && ClassifyTarget(targetPath) == WrapperKind.Ours)
        {
            try
            {
                File.Delete(targetPath);
                ConsoleUx.Warn(LogComponent.Patch, "Recovery: orphan VRCResolver wrapper deleted from Tools (no backup to restore from). VRChat will re-download its yt-dlp on next session.");
            }
            catch (Exception ex)
            {
                ConsoleUx.Warn(LogComponent.Patch, "Recovery: orphan deletion failed: " + ex.Message);
            }
        }
    }

    public bool Start()
    {
        if (Interlocked.Exchange(ref _started, 1) != 0)
            return true;

        if (string.IsNullOrEmpty(_vrcToolsDir))
        {
            ConsoleUx.Error(LogComponent.Patch, "Cannot apply patch -- VRChat Tools folder not found. Launch VRChat once first, then re-run.");
            Interlocked.Exchange(ref _started, 0);
            return false;
        }
        if (!File.Exists(_patchedYtDlpPath))
        {
            ConsoleUx.Error(LogComponent.Patch, "Cannot apply patch -- patched yt-dlp.exe is missing from this install. Reinstall vrcresolver.");
            Interlocked.Exchange(ref _started, 0);
            return false;
        }

        string targetPath = Path.Combine(_vrcToolsDir, "yt-dlp.exe");
        string backupPath = Path.Combine(_vrcToolsDir, "yt-dlp-og.exe");
        if (!File.Exists(targetPath) && !File.Exists(backupPath))
        {
            ConsoleUx.Error(LogComponent.Patch, "Cannot apply patch -- VRChat hasn't shipped its own yt-dlp.exe yet, and we have no original to preserve as fallback. Launch VRChat once first, then re-run.");
            Interlocked.Exchange(ref _started, 0);
            return false;
        }

        try { if (File.Exists(_haltFlagPath)) File.Delete(_haltFlagPath); }
        catch { }

        _loop = Task.Run(WatchdogLoop);
        return true;
    }

    public async Task StopAsync()
    {
        if (Interlocked.Exchange(ref _stopping, 1) != 0) return;

        _cts.Cancel();
        if (_loop != null)
        {
            try { await _loop.ConfigureAwait(false); } catch { }
        }

        bool cleanShutdown;
        if (Volatile.Read(ref _started) == 0)
        {
            cleanShutdown = true;
        }
        else if (string.IsNullOrEmpty(_vrcToolsDir))
        {
            cleanShutdown = true;
        }
        else
        {
            cleanShutdown = RestoreYtDlpInTools(_vrcToolsDir);
            ToolsDirSweeper.Sweep(_vrcToolsDir);
        }

        if (cleanShutdown)
        {
            try { File.WriteAllText(_cleanExitFlagPath, DateTime.UtcNow.ToString("o")); }
            catch { }
        }
    }

    private async Task WatchdogLoop()
    {
        while (!_cts.IsCancellationRequested)
        {
            try { TickOnce(); }
            catch (Exception ex) { ConsoleUx.Warn(LogComponent.Patch, "tick error: " + ex.Message); }
            try { await Task.Delay(TickDelayMs, _cts.Token).ConfigureAwait(false); }
            catch (OperationCanceledException) { break; }
        }
    }

    private void TickOnce()
    {
        if (_halted) return;
        if (string.IsNullOrEmpty(_vrcToolsDir)) return;

        string targetPath = Path.Combine(_vrcToolsDir, "yt-dlp.exe");
        string backupPath = Path.Combine(_vrcToolsDir, "yt-dlp-og.exe");

        bool targetExists = File.Exists(targetPath);
        bool backupExists = File.Exists(backupPath);

        if (!backupExists)
        {
            if (!targetExists)
            {
                EmitTickStateChange(TickOutcome.Waiting,
                    "[patch] tick: VRChat has not installed yt-dlp yet -- waiting");
                return;
            }

            WrapperKind kind = ClassifyTarget(targetPath);
            if (kind == WrapperKind.VrcBundledYtDlp)
            {
                if (IsTargetInUse(targetPath))
                {
                    EmitTickStateChange(TickOutcome.Locked,
                        "[patch] tick: yt-dlp.exe locked (VRChat may be mid-CreateProcess) -- deferring backup creation");
                    return;
                }
                try
                {
                    File.Move(targetPath, backupPath);
                    EmitTickStateChange(TickOutcome.BackupCreated,
                        "[patch] tick: preserved VRChat's yt-dlp.exe as yt-dlp-og.exe");
                }
                catch (IOException) { return; }
                targetExists = false;
                backupExists = true;
            }
            else if (kind == WrapperKind.Ours)
            {
                if (IsTargetInUse(targetPath))
                {
                    EmitTickStateChange(TickOutcome.Locked,
                        "[patch] tick: yt-dlp.exe locked -- deferring recovery delete");
                    return;
                }
                try
                {
                    File.Delete(targetPath);
                    EmitTickStateChange(TickOutcome.BackupLost,
                        "[patch] tick: yt-dlp-og.exe is missing and target is our wrapper -- removed the wrapper so VRChat redownloads its yt-dlp");
                }
                catch (IOException ex) { ReportLockFailure("recovery-delete", ex); }
                return;
            }
            else
            {
                EmitTickStateChange(TickOutcome.UnknownTarget,
                    "[patch] tick: Tools/yt-dlp.exe is not classified as ours or VRChat-bundled -- not mutating");
                return;
            }
        }

        if (!File.Exists(_patchedYtDlpPath))
        {
            Halt("patched_binary_missing");
            return;
        }

        if (!targetExists)
        {
            if (IsTargetInUse(targetPath))
            {
                EmitTickStateChange(TickOutcome.Locked,
                    "[patch] tick: yt-dlp.exe locked at initial-stage -- deferring");
                return;
            }
            try
            {
                AtomicCopy(_patchedYtDlpPath, targetPath);
                _lastPatchTime = DateTime.UtcNow;
                _consecutiveLockFailures = 0;
                EmitTickStateChange(TickOutcome.InitialStaged,
                    "[patch] tick: wrapper installed at " + targetPath);
            }
            catch (IOException ex) { ReportLockFailure("initial-stage", ex); }
            return;
        }

        WrapperKind targetKind = ClassifyTarget(targetPath);
        if (targetKind == WrapperKind.Ours)
        {
            _consecutiveLockFailures = 0;
            EmitTickStateChangeFileOnly(TickOutcome.Match,
                "[patch] tick: target is our wrapper, no action");
            return;
        }

        if ((DateTime.UtcNow - _lastPatchTime).TotalSeconds < MinReapplyGapSec) return;

        if (targetKind == WrapperKind.VrcBundledYtDlp)
        {
            if (IsTargetInUse(targetPath))
            {
                EmitTickStateChange(TickOutcome.Locked,
                    "[patch] tick: yt-dlp.exe locked (VRChat or yt-dlp running) -- deferring re-apply");
                return;
            }
            try
            {
                AtomicCopy(targetPath, backupPath);
                AtomicCopy(_patchedYtDlpPath, targetPath);
                _lastPatchTime = DateTime.UtcNow;
                _consecutiveLockFailures = 0;
                EmitTickStateChange(TickOutcome.Reapplied,
                    "[patch] yt-dlp.exe was updated by VRChat -- refreshed yt-dlp-og.exe, wrapper re-applied.");
            }
            catch (IOException ex)
            {
                ReportLockFailure("re-apply", ex);
                EmitTickStateChange(TickOutcome.ReapplyFailed,
                    "[patch] tick: re-apply failed (sharing violation) -- retry next tick");
            }
            return;
        }

        EmitTickStateChange(TickOutcome.UnknownTarget,
            "[patch] tick: target is neither our wrapper nor VRChat-bundled -- not mutating");
    }

    internal static bool IsTargetInUse(string path)
    {
        try
        {
            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.None);
            return false;
        }
        catch (FileNotFoundException) { return false; }
        catch (DirectoryNotFoundException) { return false; }
        catch (UnauthorizedAccessException) { return false; }
        catch (IOException ex) when ((uint)ex.HResult == 0x80070020) { return true; }
        catch (IOException) { return true; }
        catch { return false; }
    }

    private void EmitTickStateChange(TickOutcome outcome, string message)
    {
        if (_lastTickOutcome == outcome) return;
        _lastTickOutcome = outcome;
        ConsoleUx.Write(LogComponent.Patch, StripPatchPrefix(message));
    }

    private void EmitTickStateChangeFileOnly(TickOutcome outcome, string message)
    {
        if (_lastTickOutcome == outcome) return;
        _lastTickOutcome = outcome;
        Logger.WriteFileOnly(message);
    }

    private static string StripPatchPrefix(string message)
    {
        const string prefix = "[patch] ";
        return message != null && message.StartsWith(prefix, StringComparison.Ordinal)
            ? message[prefix.Length..]
            : (message ?? "");
    }

    private int _consecutiveLockFailures;
    private void ReportLockFailure(string stage, IOException ex)
    {
        _consecutiveLockFailures++;
        if (_consecutiveLockFailures == 3)
        {
            ConsoleUx.Warn(LogComponent.Patch, "" + stage + " has failed 3 times in a row -- possible antivirus interference or permissions issue. Last error: "
                + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 120));
        }
        else if (_consecutiveLockFailures > 3 && _consecutiveLockFailures % 20 == 0)
        {
            ConsoleUx.Warn(LogComponent.Patch, "" + stage + " still failing after " + _consecutiveLockFailures + " ticks ("
                + ex.GetType().Name + ": " + LogUtil.SanitizeForConsole(ex.Message, 120) + ")");
        }
    }

    private void Halt(string reason)
    {
        _halted = true;
        bool restored = false;
        if (!string.IsNullOrEmpty(_vrcToolsDir))
        {
            try { restored = RestoreYtDlpInTools(_vrcToolsDir); }
            catch (Exception ex) { ConsoleUx.Warn(LogComponent.Patch, "halt restore threw: " + ex.Message); }
            ToolsDirSweeper.Sweep(_vrcToolsDir);
        }

        ConsoleUx.Fatal("VRCResolver halted -- Reinstall VRCResolver; reason=" + reason + " restored=" + restored);
        try { Console.Title = "VRCResolver -- HALTED (" + reason + ")"; } catch { }

        try { File.WriteAllText(_haltFlagPath, DateTime.UtcNow.ToString("o") + " " + reason); }
        catch (Exception ex) { ConsoleUx.Warn(LogComponent.Patch, "could not write halt.flag: " + ex.Message); }
        _cts.Cancel();
    }

    internal static void AtomicCopy(string src, string dst)
    {
        string tmp = dst + ".new-" + Guid.NewGuid().ToString("N").Substring(0, 8);
        try
        {
            File.Copy(src, tmp, overwrite: true);
            File.Move(tmp, dst, overwrite: true);
        }
        catch
        {
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { }
            throw;
        }
    }

    public static bool RestoreYtDlpInTools(string toolsDir)
    {
        if (string.IsNullOrEmpty(toolsDir) || !Directory.Exists(toolsDir)) return false;
        string targetPath = Path.Combine(toolsDir, "yt-dlp.exe");
        string backupPath = Path.Combine(toolsDir, "yt-dlp-og.exe");
        if (!File.Exists(backupPath)) return false;

        try
        {
            try
            {
                File.Move(backupPath, targetPath, overwrite: true);
                return true;
            }
            catch (IOException)
            {
            }

            if (File.Exists(targetPath))
            {
                string stale = targetPath + ".stale-" + DateTime.UtcNow.ToString("yyyyMMddHHmmssfff");
                File.Move(targetPath, stale);
                ConsoleUx.Write(LogComponent.Patch, "yt-dlp.exe was locked; moved aside to " + Path.GetFileName(stale) + ".");
            }
            File.Move(backupPath, targetPath);
            return true;
        }
        catch (Exception ex)
        {
            ConsoleUx.Warn(LogComponent.Patch, "restore error: " + ex.Message);
            return false;
        }
    }

    public void Dispose()
    {
        _cts.Cancel();
        _cts.Dispose();
    }
}
