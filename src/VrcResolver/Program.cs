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
    private static ShutdownCoordinator? s_shutdown;
    private static readonly ManualResetEventSlim s_quitSignal = new(false);
    private static volatile bool s_fastShutdown;
    private static int s_shutdownStarted;

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
        Logger.WriteFileOnly("[console] quick_edit=" + TerminalCapabilities.DisableQuickEdit());

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
            BeginShutdown("ctrl-c", fast: false);
            s_quitSignal.Set();
        };
        AppDomain.CurrentDomain.ProcessExit += (_, _) =>
        {
            Task shutdown = BeginShutdown("process-exit", fast: true);
            s_quitSignal.Set();
            try { shutdown.Wait(TimeSpan.FromMilliseconds(2000)); }
            catch { }
        };
        AppDomain.CurrentDomain.UnhandledException += (_, _) =>
        {
            try { BeginShutdown("unhandled-exception", fast: true).Wait(TimeSpan.FromSeconds(3)); }
            catch { }
        };
        if (!SetConsoleCtrlHandler(s_ctrlHandler, true))
            ConsoleUx.Warn(LogComponent.Shutdown, "could not hook console close events; if this window is closed, VRChat's tools are put back on the next start.");

        Logger.WriteFileOnly("[startup] parent=" + (ParentProcess.Describe() ?? "unknown"));

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

        PatchManager patcher = new(installDir);
        s_patcher = patcher;
        s_shutdown = new ShutdownCoordinator(
            trigger => RestoreToolsFirst(patcher, trigger),
            ShutdownSteps,
            line => Logger.WriteFileOnly("[shutdown] " + line));
        patcher.RecoverFromUncleanShutdown();

        s_mesh = new MeshClient();
        s_resolveCache = new ResolveCache();
        s_ogFallbackHint = new OgFallbackHint();
        s_healthGate = new ResolverHealthGate();
        s_ipc = new LocalIpcServer(s_mesh, s_resolveCache, s_ogFallbackHint, s_healthGate);
        s_ipc.Start();
        _ = s_mesh.StartAsync();

        try { WinInetConnectionLimit.Ensure(msg => ConsoleUx.Write(LogComponent.Relay, msg)); }
        catch (Exception ex) { ConsoleUx.Warn(LogComponent.Relay, "could not raise WinINet connection limit: " + ex.Message); }

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

        RelayBypass? relayBypass = s_relay != null ? new RelayBypass(s_relayPort, s_relay.Scheme) : null;
        s_logmon = new VrcLogMonitor(
            s_mesh,
            s_resolveCache,
            s_ogFallbackHint,
            s_healthGate,
            relayBypass == null ? null : relayBypass.Engage,
            relayBypass == null ? null : relayBypass.Release);
        s_logmon.Start();

        if (!patcher.Start())
        {
            BeginShutdown("patch-refused", fast: false).GetAwaiter().GetResult();
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

        BeginShutdown("quit", fast: false).GetAwaiter().GetResult();
        return 0;
    }

    private static bool OnConsoleCtrl(uint ctrlType)
    {
        string? trigger = ctrlType switch
        {
            CTRL_C_EVENT or CTRL_BREAK_EVENT => "ctrl-c",
            CTRL_CLOSE_EVENT => "console-close",
            CTRL_LOGOFF_EVENT => "logoff",
            CTRL_SHUTDOWN_EVENT => "system-shutdown",
            _ => null,
        };
        if (trigger == null) return false;

        if (ctrlType is CTRL_C_EVENT or CTRL_BREAK_EVENT)
        {
            BeginShutdown(trigger, fast: false);
            s_quitSignal.Set();
            return true;
        }

        try { BeginShutdown(trigger, fast: true).Wait(TimeSpan.FromMilliseconds(4500)); }
        catch { }
        s_quitSignal.Set();
        return true;
    }

    private static Task BeginShutdown(string trigger, bool fast)
    {
        ShutdownCoordinator? shutdown = s_shutdown;
        if (shutdown == null) return Task.CompletedTask;
        if (Interlocked.Exchange(ref s_shutdownStarted, 1) == 0) s_fastShutdown = fast;
        return shutdown.RunAsync(trigger, fast);
    }

    private static bool RestoreToolsFirst(PatchManager patcher, string trigger)
    {
        bool restored = patcher.RestoreNow(trigger);
        s_relayPort?.DeletePortFile();
        return restored;
    }

    private static IReadOnlyList<ShutdownStep> ShutdownSteps()
    {
        var steps = new List<ShutdownStep>();
        if (s_terminal is { } terminal)
            steps.Add(new ShutdownStep("terminal", terminal.StopAsync, TimeSpan.FromMilliseconds(500)));
        if (s_heartbeat is { } heartbeat)
            steps.Add(new ShutdownStep("heartbeat", heartbeat.StopAsync, TimeSpan.FromMilliseconds(500)));
        if (s_hostsTicker is { } hostsTicker)
            steps.Add(new ShutdownStep("hosts-ticker", hostsTicker.StopAsync, TimeSpan.FromMilliseconds(500)));
        if (s_logmon is { } logmon)
            steps.Add(new ShutdownStep("logmon", logmon.StopAsync, TimeSpan.FromMilliseconds(1000)));
        if (s_ipc is { } ipc)
            steps.Add(new ShutdownStep("ipc", ipc.StopAsync, TimeSpan.FromMilliseconds(3000)));
        if (s_relay is { } relay)
        {
            steps.Add(new ShutdownStep("relay", relay.StopAsync, TimeSpan.FromMilliseconds(2000)));
            steps.Add(new ShutdownStep("relay-dispose", () =>
            {
                relay.Dispose();
                s_relay = null;
                return Task.CompletedTask;
            }, TimeSpan.FromMilliseconds(500)));
        }
        if (s_mesh is { } mesh)
            steps.Add(new ShutdownStep("mesh", mesh.StopAsync, TimeSpan.FromMilliseconds(3000)));
        if (s_resolveCache is { } resolveCache)
        {
            steps.Add(new ShutdownStep("resolve-cache", () =>
            {
                resolveCache.FlushNow();
                return Task.CompletedTask;
            }, TimeSpan.FromMilliseconds(1000)));
        }
        return steps;
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
            sb.AppendLine("patch:   vrcToolsDir=" + (patcher.VrcToolsDir ?? "<null>"));
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
