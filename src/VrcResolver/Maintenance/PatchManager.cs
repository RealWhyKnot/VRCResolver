using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

// Keeps VRChat's Tools\yt-dlp.exe pointed at the patched build that talks to
// our pipe, while preserving VRChat's bundled yt-dlp (a modified upstream
// distribution VRChat maintains, not vanilla yt-dlp) at yt-dlp-og.exe so the
// wrapper can exec it on resolve failure.
//
// Identification uses WrapperIdentity.Classify (marker + PE metadata + known
// release hashes + size band) so older wrappers from prior
// installs and dev builds are never confused with the VRChat-bundled yt-dlp.
//
// Behaviour:
//   - On every 3s tick, classify whatever is at Tools/yt-dlp.exe and act on
//     the three cases (Ours / VrcBundledYtDlp / Unknown).
//   - If VRChat hasn't installed its yt-dlp yet, wait silently for it to
//     appear -- never substitute a vanilla copy from elsewhere.
//   - If the og backup goes missing but Tools/yt-dlp.exe is our wrapper,
//     remove the wrapper so VRChat re-downloads its modified yt-dlp; we
//     never want to leave VRChat with our wrapper as the only file in Tools
//     and no upstream copy preserved.
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
    private int _started;  // Interlocked: 0 = not started, 1 = started
    private int _stopping; // Interlocked: 0 = idle, 1 = stop in flight / done

    // Single-slot classify cache. WrapperIdentity.Classify reads up to 16 MiB
    // to scan for the embedded marker; running it on every 3s tick would
    // burn ~19 GiB/h of disk reads on an idle watchdog. The (path, size,
    // mtime) tuple keys the cache -- any of those changing invalidates and
    // forces a re-classify.
    private string? _classifyCachePath;
    private long _classifyCacheSize;
    private DateTime _classifyCacheMtime;
    private WrapperKind _classifyCacheKind;

    // Last decision the watchdog logged. Tick logging is state-change-gated
    // so a sustained "no action" loop doesn't fill scrollback every 3 s.
    // Distinct values mirror TickOutcome below.
    private TickOutcome _lastTickOutcome = TickOutcome.None;

    private enum TickOutcome { None, Match, Locked, Reapplied, ReapplyFailed, BackupCreated, InitialStaged, Waiting, UnknownTarget, BackupLost }

    public string? VrcToolsDir => _vrcToolsDir;
    public bool Halted => _halted;

    // One-shot startup status line. If VRChat is already running, the patch
    // tick will defer the first-rename until VRChat releases its handle —
    // this banner makes that explicit so the operator isn't confused by the
    // delay. PID + start time printed for correlation with the per-tick
    // "deferring" log lines that may follow.
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
            // Pick the oldest (most likely the actual game; auxiliary tools
            // sometimes spawn briefly-named "VRChat" helpers).
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
                catch { /* StartTime can throw on access-denied; fall through */ }
            }
            string startedStr;
            try { startedStr = primary.StartTime.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ"); }
            catch { startedStr = "<unknown>"; }
            ConsoleUx.Write(
                LogComponent.Patch,
                "VRChat is currently running (PID " + primary.Id +
                ", started " + startedStr + ") -- patch will apply when yt-dlp.exe isn't actively in use.");
            foreach (var p in procs) try { p.Dispose(); } catch { /* best-effort */ }
        }
        catch (Exception ex)
        {
            // Process enumeration can fail in locked-down contexts (RDP
            // session privileges, AV interference). Don't escalate; the
            // 3-s tick loop's lock probe handles correctness regardless.
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

    // Run once at startup before Start(). If the previous shutdown was unclean,
    // try to put the Tools folder back into a sane state so the watchdog can
    // engage from a known baseline.
    public void RecoverFromUncleanShutdown()
    {
        // Always sweep our sidecars first, regardless of clean/unclean flag.
        // .new-<short> tmps from a kill-mid-AtomicCopy and .stale-<utc>
        // rename-asides accumulate across runs otherwise.
        ToolsDirSweeper.Sweep(_vrcToolsDir);

        bool cleanLastTime = File.Exists(_cleanExitFlagPath);
        if (cleanLastTime)
        {
            try { File.Delete(_cleanExitFlagPath); } catch { /* best-effort */ }
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
            // Orphan: yt-dlp.exe is our wrapper AND there's no og to restore
            // from. Delete it so VRChat will re-download its modified yt-dlp
            // the next time it tries to play a video. Start()'s refuse-to-
            // apply guard then trips cleanly with the "Launch VRChat once
            // first" message.
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
        // H15: Interlocked guard so a concurrent second caller (e.g. signal
        // handler racing main flow) can't spawn a parallel WatchdogLoop. We
        // return true on the redundant call so the caller doesn't interpret
        // "already running" as "refuse-to-apply" and trigger a shutdown.
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

        // We've confirmed both the patched binary and a Tools-side state we can
        // engage with — clear any leftover halt.flag from a prior corrupted run.
        try { if (File.Exists(_haltFlagPath)) File.Delete(_haltFlagPath); }
        catch { /* best-effort */ }

        _loop = Task.Run(WatchdogLoop);
        return true;
    }

    public async Task StopAsync()
    {
        // H16: Interlocked guard so a concurrent second StopAsync (the existing
        // outer Program.RunShutdown gate prevents this from inside the watchdog,
        // but a different caller path — e.g. crash-handler invoking StopAsync
        // directly — could otherwise race two parallel restores.
        if (Interlocked.Exchange(ref _stopping, 1) != 0) return;

        _cts.Cancel();
        if (_loop != null)
        {
            try { await _loop.ConfigureAwait(false); } catch { /* ignore */ }
        }

        // Only write clean_exit.flag when the post-shutdown state is genuinely
        // clean. If we ever engaged the watchdog, the restore must have actually
        // succeeded for the flag to be honest. If it didn't (og missing, target
        // locked through every retry, etc.) we leave the flag absent so the
        // next launch's RecoverFromUncleanShutdown gets a chance to fix it.
        bool cleanShutdown;
        if (Volatile.Read(ref _started) == 0)
        {
            // Watchdog never engaged — Tools dir was untouched by us this run.
            cleanShutdown = true;
        }
        else if (string.IsNullOrEmpty(_vrcToolsDir))
        {
            cleanShutdown = true;
        }
        else
        {
            cleanShutdown = RestoreYtDlpInTools(_vrcToolsDir);
            // Sweep our own sidecars whether the restore succeeded or not.
            // The .stale-<utc> file produced by the locked-target branch of
            // RestoreYtDlpInTools is exactly the kind of leftover this is
            // here to clean up.
            ToolsDirSweeper.Sweep(_vrcToolsDir);
        }

        if (cleanShutdown)
        {
            try { File.WriteAllText(_cleanExitFlagPath, DateTime.UtcNow.ToString("o")); }
            catch { /* best-effort */ }
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
                // Both missing -- waiting for VRChat to install its yt-dlp.
                // No mutation; next tick re-checks. This is a normal state
                // for a fresh VRChat install or a freshly-wiped Tools dir.
                EmitTickStateChange(TickOutcome.Waiting,
                    "[patch] tick: VRChat has not installed yt-dlp yet -- waiting");
                return;
            }

            WrapperKind kind = ClassifyTarget(targetPath);
            if (kind == WrapperKind.VrcBundledYtDlp)
            {
                // First-run rename: VRChat-bundled yt-dlp is sitting at
                // target, preserve it as the og backup. Lock-probe so we
                // don't race a VRChat CreateProcess mid-mapping the PE
                // sections (would land VRChat with a half-loaded image).
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
                // Fall through to the initial-stage branch below.
                targetExists = false;
                backupExists = true;
            }
            else if (kind == WrapperKind.Ours)
            {
                // Our wrapper is at target but the backup is gone. We can't
                // leave VRChat with only our wrapper and no upstream copy --
                // there'd be nothing to exec on fallback. Delete the wrapper
                // so VRChat re-downloads its modified yt-dlp on the next
                // session; the resulting both-missing state hits the Waiting
                // branch on the next tick. Probe for locks first to honour
                // the never-crash-VRChat invariant.
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
                // Unknown: don't touch. Some other tool may have put a file
                // here we don't recognize. The wait-and-watch policy says
                // we wait until state changes to something classifiable.
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
            // Initial stage. Target doesn't exist (we just renamed it away
            // in the first-run branch above, or it never existed). No active
            // CreateProcess to race with on a non-existent path.
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

        // Target exists alongside a backup. Classify it.
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
            // VRChat (or its auto-updater) replaced our wrapper with a new
            // bundled yt-dlp. Refresh the backup to the new copy BEFORE
            // re-staging our wrapper so the fallback chain reflects what
            // VRChat currently ships, then re-apply.
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

        // targetKind == Unknown. Don't overwrite something we can't
        // identify -- wait until the next tick sees a classifiable state.
        EmitTickStateChange(TickOutcome.UnknownTarget,
            "[patch] tick: target is neither our wrapper nor VRChat-bundled -- not mutating");
    }

    // Probe yt-dlp.exe with FileShare.None. If we acquire exclusive read,
    // nobody else has it open and it's safe to swap. If we get a sharing
    // violation, VRChat is either mid-CreateProcess (kernel mapping the
    // PE sections into a child) or its own auto-updater has the file
    // open for write. In either case, swapping NOW could corrupt VRChat's
    // view of the binary and crash it. Defer.
    //
    // Missing-file / missing-dir → return false so the caller's normal
    // File.Exists / IOException paths handle them. UnauthorizedAccess and
    // other non-sharing-violation IOExceptions also fall through as "not
    // in use" — we don't want to defer indefinitely on a permissions
    // problem. The only "true" case is a sharing-violation IOException,
    // which Windows raises with HRESULT 0x80070020 (ERROR_SHARING_VIOLATION).
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

    // Emit a tick decision line only when the outcome differs from the
    // previous tick's. Sustained "no action" stretches stay quiet; the
    // first lock / first re-apply / first match-after-mismatch each get
    // exactly one log line, with state recovery transitions also logged.
    private void EmitTickStateChange(TickOutcome outcome, string message)
    {
        if (_lastTickOutcome == outcome) return;
        _lastTickOutcome = outcome;
        ConsoleUx.Write(LogComponent.Patch, StripPatchPrefix(message));
    }

    // File-only variant for the steady-state Match outcome. Demoted in
    // the logging audit (redundant with the "wrapper installed at <path>"
    // line that fires when the patch first goes in). Same gating: only
    // emits on outcome transition, not every tick.
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

    // Track consecutive IOException ticks so a sustained AV-lock or
    // permission-flip is visible in the console (pre-fix it was completely
    // silent — the watchdog appeared to be running but wasn't actually
    // applying anything).
    private int _consecutiveLockFailures;
    private void ReportLockFailure(string stage, IOException ex)
    {
        _consecutiveLockFailures++;
        // First failure: silent (transient is normal). Third failure: warn.
        // Then every 20th (~1 minute at 3s ticks) so an indefinite stall
        // is visible without filling the scrollback.
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

    // Halt the watchdog loop. ALWAYS attempts to leave VRChat with a working
    // yt-dlp.exe before stopping ticks — restoring from yt-dlp-og.exe if it
    // still exists. On a successful restore VRChat falls back to vanilla
    // yt-dlp behaviour the next time it launches a video. Persists a halt.flag
    // (with reason + timestamp) so a future maintenance pass can detect that
    // the daemon is alive but no longer functional.
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

        // Halt banner: bracket the message with a divider so it stands out
        // in the scrollback and update the console window title so a user
        // glancing at the taskbar sees the daemon is no longer functional
        // even after the message has scrolled off.
        ConsoleUx.Fatal("VRCResolver halted -- Reinstall VRCResolver; reason=" + reason + " restored=" + restored);
        try { Console.Title = "VRCResolver -- HALTED (" + reason + ")"; } catch { /* best-effort */ }

        try { File.WriteAllText(_haltFlagPath, DateTime.UtcNow.ToString("o") + " " + reason); }
        catch (Exception ex) { ConsoleUx.Warn(LogComponent.Patch, "could not write halt.flag: " + ex.Message); }
        _cts.Cancel();
    }

    // Copies src over dst with no partial-file window: stage to a sibling
    // tmp on dst's volume, then atomic same-volume rename. A kill mid-copy
    // leaves dst untouched (still vanilla or still patched, but never
    // truncated). The tmp file is cleaned up on any failure path.
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
            try { if (File.Exists(tmp)) File.Delete(tmp); } catch { /* best-effort */ }
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
            // Fast path: atomic same-volume rename. No window where target is missing.
            try
            {
                File.Move(backupPath, targetPath, overwrite: true);
                return true;
            }
            catch (IOException)
            {
                // Target is locked (VRChat / AV holding it open). Move it aside, then retry.
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
