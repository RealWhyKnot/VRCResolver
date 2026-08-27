using System.Diagnostics;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using VrcResolver.Shared;

namespace VrcResolver;

[SupportedOSPlatform("windows")]
internal static class Program
{
    private const string MutexName = "Global\\vrcresolver.Watchdog";
    private const string MutexNameLocal = "Local\\vrcresolver.Watchdog";

    private static LocalIpcServer? s_ipc;
    private static MeshClient? s_mesh;
    private static PatchManager? s_patcher;
    private static VrcLogMonitor? s_logmon;
    private static HostsTicker? s_hostsTicker;
    private static Heartbeat? s_heartbeat;
    private static ResolveCache? s_resolveCache;
    private static OgFallbackHint? s_ogFallbackHint;
    private static ResolverHealthGate? s_healthGate;
    private static RelayPortManager? s_relayPort;
    private static LocalRelayServer? s_relay;
    private static InteractiveTerminal? s_terminal;
    private static readonly ManualResetEventSlim s_quitSignal = new(false);
    private static volatile bool s_fastShutdown;

    [DllImport("Kernel32")]
    private static extern bool SetConsoleCtrlHandler(ConsoleCtrlDelegate? handler, bool add);
    private delegate bool ConsoleCtrlDelegate(uint ctrlType);
    private const uint CTRL_C_EVENT = 0;
    private const uint CTRL_BREAK_EVENT = 1;
    private const uint CTRL_CLOSE_EVENT = 2;
    private const uint CTRL_LOGOFF_EVENT = 5;
    private const uint CTRL_SHUTDOWN_EVENT = 6;

    private static readonly ConsoleCtrlDelegate s_ctrlHandler = OnConsoleCtrl;

    private static int Main(string[] args)
    {
        try { Console.OutputEncoding = System.Text.Encoding.UTF8; } catch { }

        AppPaths.MigrateFromLegacyProduct(Console.WriteLine);

        Logger.Install("watchdog");
        Logger.SetDevConsoleDiagnostics(BuildInfo.IsDevBuild);

        ReportingService.Initialize();

        CrashHandler.Install("watchdog");

        if (args.Length > 0)
        {
            switch (args[0])
            {
                case HostsManager.AddArg: return HostsManager.RunAddInElevatedChild();
                case HostsManager.RemoveArg: return HostsManager.RunRemoveInElevatedChild();
                case LocalRelayTlsManager.BootstrapArg: return LocalRelayTlsManager.RunBootstrapInElevatedChild(args);
                case LocalRelayTlsManager.RemoveArg: return LocalRelayTlsManager.RunRemoveInElevatedChild();
            }
        }

        System.Threading.Mutex? mutex = null;
        System.Threading.Mutex? legacyMutex = null;
        try
        {
            try
            {
                mutex = new System.Threading.Mutex(false, MutexName, out _);
                legacyMutex = new System.Threading.Mutex(false, LegacyCompat.LegacyWatchdogMutexName, out _);
            }
            catch (UnauthorizedAccessException)
            {
                ConsoleUx.Warn(LogComponent.Terminal, "could not create global mutex; using session-local mutex.");
                mutex ??= new System.Threading.Mutex(false, MutexNameLocal, out _);
                legacyMutex = new System.Threading.Mutex(false, LegacyCompat.LegacyWatchdogMutexNameLocal, out _);
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine("Mutex creation failed: " + ex.GetType().Name + ": " + ex.Message);
                return 3;
            }

            bool acquired = false;
            try { acquired = mutex.WaitOne(TimeSpan.Zero); }
            catch (AbandonedMutexException) { acquired = true; }

            bool legacyAcquired = false;
            try { legacyAcquired = legacyMutex.WaitOne(TimeSpan.Zero); }
            catch (AbandonedMutexException) { legacyAcquired = true; }

            if (!acquired || !legacyAcquired)
            {
                ConsoleUx.Warn(LogComponent.Terminal, "VRCResolver is already running.");
                if (acquired) { try { mutex.ReleaseMutex(); } catch { } }
                if (legacyAcquired) { try { legacyMutex.ReleaseMutex(); } catch { } }
                return 1;
            }

            try
            {
                UpdaterRepair.ApplyIfPresent(AppContext.BaseDirectory);
                return RunWatchdog();
            }
            finally
            {
                try { mutex.ReleaseMutex(); } catch { }
                try { legacyMutex.ReleaseMutex(); } catch { }
            }
        }
        finally
        {
            mutex?.Dispose();
            legacyMutex?.Dispose();
        }
    }

    private static int RunWatchdog()
    {
        Console.CancelKeyPress += (_, e) =>
        {
            e.Cancel = true;
            s_quitSignal.Set();
        };
        AppDomain.CurrentDomain.ProcessExit += (_, _) =>
        {
            s_fastShutdown = true;
            s_quitSignal.Set();
            try { RunShutdown().Wait(TimeSpan.FromMilliseconds(2000)); }
            catch { }
        };
        AppDomain.CurrentDomain.UnhandledException += (_, _) =>
        {
            s_fastShutdown = true;
            try { RunShutdown().Wait(TimeSpan.FromSeconds(3)); }
            catch { }
        };
        SetConsoleCtrlHandler(s_ctrlHandler, true);

        CrashHandler.SetStateSnapshot(SnapshotState);

        var version = Assembly.GetExecutingAssembly().GetName().Version?.ToString() ?? "0.0.0.0";
        string installDir = AppContext.BaseDirectory;
        string stateDir = AppPaths.StateRoot();
        string vrcToolsDir = VrcPathLocator.Find() ?? "<not found — launch VRChat once>";

#pragma warning disable CS0162
        bool isDev = BuildInfo.IsDevBuild;
#pragma warning restore CS0162
        AppSettings settings = AppSettingsStore.Shared.Snapshot();
        ConsoleUx.Banner(
            version: version,
            sha: BuildInfo.GitSha,
            buildTime: BuildInfo.BuildTime,
            isDev: isDev,
            paths: new (string, string)[]
            {
                ("install",   installDir),
                ("vrc tools", vrcToolsDir),
                ("state",     stateDir),
                ("os",        RuntimeInformation.OSDescription),
                ("runtime",   RuntimeInformation.FrameworkDescription),
            });

        PatchManager.LogVrcProcessState();

        s_patcher = new PatchManager(installDir);
        s_patcher.RecoverFromUncleanShutdown();

        s_mesh = new MeshClient();
        s_resolveCache = new ResolveCache();
        s_ogFallbackHint = new OgFallbackHint();
        s_healthGate = new ResolverHealthGate();
        s_ipc = new LocalIpcServer(s_mesh, s_resolveCache, s_ogFallbackHint, s_healthGate);
        s_ipc.Start();
        _ = s_mesh.StartAsync();

        s_relayPort = new RelayPortManager();
        if (s_relayPort.Initialize())
        {
            bool relayHttpsAllowed = !string.Equals(
                settings.Relay.Https,
                RelayAppSettings.HttpsOff,
                StringComparison.OrdinalIgnoreCase);
            if (!relayHttpsAllowed)
                ConsoleUx.Write(LogComponent.Relay, "secure local video disabled in settings; using local HTTP fallback.");

            if (!TryStartLocalRelay(s_relayPort, relayHttpsAllowed))
            {
                ConsoleUx.Warn(LogComponent.Relay, "local video relay could not start -- public-instance local video disabled.");
            }
        }
        else
        {
            ConsoleUx.Warn(LogComponent.Relay, "could not reserve a local video port -- public-instance local video disabled.");
        }

        s_logmon = new VrcLogMonitor(s_mesh, s_resolveCache, s_ogFallbackHint, s_healthGate);
        s_logmon.Start();

        if (!s_patcher.Start())
        {
            RunShutdown().GetAwaiter().GetResult();
            return 2;
        }

        ConsoleUx.Success(LogComponent.Patch, "VRChat video hook ready; watching for game updates.");
        ConsoleUx.Write(LogComponent.Terminal, "type /help for commands, /status for activity, /settings for options.");
        ConsoleUx.Write(LogComponent.Terminal, "to uninstall, run vrcresolver.Uninstaller.exe from this folder.");

        _ = Task.Run(() =>
        {
            try { HostsManager.EnsureBypassEntryOrPrompt(); }
            catch (Exception ex) { ConsoleUx.Warn(LogComponent.Hosts, "background check failed: " + ex.Message); }
        });

        s_hostsTicker = new HostsTicker();
        s_hostsTicker.Start();

        s_heartbeat = new Heartbeat(s_mesh, s_resolveCache);
        s_heartbeat.Start();

        UpdateCheck.StartBackgroundCheck();

        CodecCapabilityProbe.Refresh();

        CodecInstaller.StartBackgroundCheck();

        ToolsDirSweeper.SweepLegacyInstallTools(AppContext.BaseDirectory);

        try
        {
            string src = Path.Combine(AppContext.BaseDirectory, "data", "wrapper_hashes.txt");
            if (File.Exists(src))
            {
                string stateRoot = AppPaths.StateRoot();
                Directory.CreateDirectory(stateRoot);
                File.Copy(src, Path.Combine(stateRoot, "wrapper_hashes.txt"), overwrite: true);
            }
        }
        catch (Exception ex)
        {
            Logger.WriteFileOnly("[startup] could not stage wrapper_hashes for wrapper: " + ex.Message);
        }

        s_terminal = new InteractiveTerminal(
            requestShutdown: () => s_quitSignal.Set(),
            meshConnected: () => s_mesh?.IsConnected == true);
        s_terminal.Start();

        s_quitSignal.Wait();
        ConsoleUx.Write(LogComponent.Shutdown, "shutting down; restoring VRChat tools and closing local services.");

        RunShutdown().GetAwaiter().GetResult();
        return 0;
    }

    private static bool OnConsoleCtrl(uint ctrlType)
    {
        switch (ctrlType)
        {
            case CTRL_C_EVENT:
            case CTRL_BREAK_EVENT:
                s_quitSignal.Set();
                return false;

            case CTRL_CLOSE_EVENT:
            case CTRL_LOGOFF_EVENT:
            case CTRL_SHUTDOWN_EVENT:
                s_fastShutdown = true;
                s_quitSignal.Set();
                try { RunShutdown().Wait(TimeSpan.FromMilliseconds(4500)); }
                catch { }
                return true;
        }
        return false;
    }

    private static bool TryStartLocalRelay(RelayPortManager relayPort, bool relayHttpsAllowed)
    {
        if (TryStartLocalRelayOnCurrentPort(relayPort, relayHttpsAllowed, out string failure))
            return true;

        if (relayPort.TryReserveFreshPort(failure)
            && TryStartLocalRelayOnCurrentPort(relayPort, relayHttpsAllowed, out _))
            return true;

        s_relay = null;
        relayPort.DeletePortFile();
        return false;
    }

    private static bool TryStartLocalRelayOnCurrentPort(
        RelayPortManager relayPort,
        bool relayHttpsAllowed,
        out string failure)
    {
        failure = "";
        string relayScheme = relayHttpsAllowed && LocalRelayTlsManager.TryEnsureReadyForPort(relayPort.CurrentPort)
            ? "https"
            : "http";
        relayPort.WriteSchemeFile(relayScheme);

        try
        {
            StartLocalRelayInstance(relayPort.CurrentPort, relayScheme);
            return true;
        }
        catch (Exception ex)
        {
            failure = ex.Message;
            if (!string.Equals(relayScheme, "https", StringComparison.OrdinalIgnoreCase))
            {
                ConsoleUx.Warn(LogComponent.Relay, "local video relay could not start on port "
                    + relayPort.CurrentPort + ": " + ex.Message);
                return false;
            }

            ConsoleUx.Warn(LogComponent.Relay, "secure local video failed: " + ex.Message
                + " -- retrying local HTTP fallback.");
            relayScheme = "http";
            relayPort.WriteSchemeFile(relayScheme);
            try
            {
                StartLocalRelayInstance(relayPort.CurrentPort, relayScheme);
                return true;
            }
            catch (Exception httpEx)
            {
                failure = httpEx.Message;
                ConsoleUx.Warn(LogComponent.Relay, "local video relay could not start on port "
                    + relayPort.CurrentPort + ": " + httpEx.Message);
                return false;
            }
        }
    }

    private static void StartLocalRelayInstance(int port, string relayScheme)
    {
        var relay = new LocalRelayServer(port, relayScheme);
        try
        {
            relay.Start();
            s_relay = relay;
            ConsoleUx.Success(LogComponent.Relay, "local video relay ready: "
                + relayScheme + "://localhost.youtube.com:" + port);
        }
        catch
        {
            relay.Dispose();
            throw;
        }
    }

    private static int s_shutdownStarted;
    private static async Task RunShutdown()
    {
        if (Interlocked.Exchange(ref s_shutdownStarted, 1) != 0) return;

        var sw = Stopwatch.StartNew();
        bool fast = s_fastShutdown;
        var totalBudget = fast ? TimeSpan.FromSeconds(4) : TimeSpan.FromSeconds(12);

        async Task WithTimeout(Task t, int ms, string step)
        {
            using var cts = new CancellationTokenSource(ms);
            var done = await Task.WhenAny(t, Task.Delay(Timeout.Infinite, cts.Token)).ConfigureAwait(false);
            if (done != t) ConsoleUx.Warn(LogComponent.Shutdown, step + " exceeded budget; moving on.");
        }

        if (!fast)
        {
            if (s_terminal != null)
            {
                try
                {
                    int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                    await WithTimeout(s_terminal.StopAsync(), Math.Min(remain, 500), "terminal").ConfigureAwait(false);
                }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "terminal: " + ex.Message); }
            }

            if (s_heartbeat != null)
            {
                try
                {
                    int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                    await WithTimeout(s_heartbeat.StopAsync(), Math.Min(remain, 500), "heartbeat").ConfigureAwait(false);
                }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "heartbeat: " + ex.Message); }
            }

            if (s_hostsTicker != null)
            {
                try
                {
                    int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                    await WithTimeout(s_hostsTicker.StopAsync(), Math.Min(remain, 500), "hosts-ticker").ConfigureAwait(false);
                }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "hosts-ticker: " + ex.Message); }
            }

            if (s_logmon != null)
            {
                try
                {
                    int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                    await WithTimeout(s_logmon.StopAsync(), Math.Min(remain, 1000), "logmon").ConfigureAwait(false);
                }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "log monitor: " + ex.Message); }
            }

            if (s_ipc != null)
            {
                try
                {
                    int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                    await WithTimeout(s_ipc.StopAsync(), Math.Min(remain, 3000), "ipc").ConfigureAwait(false);
                }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "IPC: " + ex.Message); }
            }

            if (s_relay != null)
            {
                try
                {
                    int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                    await WithTimeout(s_relay.StopAsync(), Math.Min(remain, 2000), "relay").ConfigureAwait(false);
                    s_relay.Dispose();
                    s_relay = null;
                }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "local video relay: " + ex.Message); }
            }
            try { s_relayPort?.DeletePortFile(); } catch { }

            if (s_mesh != null)
            {
                try
                {
                    int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                    await WithTimeout(s_mesh.StopAsync(), Math.Min(remain, 3000), "mesh").ConfigureAwait(false);
                }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "server connection: " + ex.Message); }
            }

            if (s_resolveCache != null)
            {
                try { s_resolveCache.FlushNow(); }
                catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "resolve cache: " + ex.Message); }
            }
        }

        try { s_relayPort?.DeletePortFile(); } catch { }

        if (s_patcher != null)
        {
            try
            {
                int remain = (int)Math.Max(0, (totalBudget - sw.Elapsed).TotalMilliseconds);
                int patcherBudget = fast ? Math.Max(remain, 3000) : Math.Min(remain, 5000);
                await WithTimeout(s_patcher.StopAsync(), patcherBudget, "patcher").ConfigureAwait(false);
            }
            catch (Exception ex) { ConsoleUx.Warn(LogComponent.Shutdown, "VRChat hook: " + ex.Message); }
        }
    }

    private static string SnapshotState()
    {
        var sb = new System.Text.StringBuilder();
        var mesh = s_mesh;
        if (mesh != null)
        {
            sb.AppendLine("mesh:    connected=" + mesh.IsConnected
                + " server_protocol_version=" + mesh.ServerProtocolVersion
                + " node=" + (mesh.ServerNode ?? "?")
                + " warp_active=" + (mesh.WarpActive?.ToString() ?? "?"));
        }
        else
        {
            sb.AppendLine("mesh:    <not constructed>");
        }
        var patcher = s_patcher;
        if (patcher != null)
        {
            sb.AppendLine("patch:   halted=" + patcher.Halted
                + " vrcToolsDir=" + (patcher.VrcToolsDir ?? "<null>"));
        }
        else
        {
            sb.AppendLine("patch:   <not constructed>");
        }
        sb.AppendLine("ipc:     " + (s_ipc != null ? "<running>" : "<not constructed>"));
        sb.AppendLine("terminal:" + (s_terminal != null ? " <running>" : " <not constructed>"));
        sb.AppendLine("shutdown_started=" + s_shutdownStarted + " fast_shutdown=" + s_fastShutdown);
        return sb.ToString();
    }
}
