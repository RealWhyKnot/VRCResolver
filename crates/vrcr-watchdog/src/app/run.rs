use std::future::Future;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Banner, Component, Ux};
use vrcr_shared::crash::Crash;
use vrcr_shared::log::Log;
use vrcr_shared::paths::AppPaths;
use vrcr_shared::win::{self, CtrlEvent, HkcuKey, MutexError, NamedMutex, RegDword};
use vrcr_shared::{legacy, tools, wininet, wire};

use super::{
    BuildInfo, CLOSE_WAIT, COMPONENT, ChildMode, CreateError, Instance, InstanceMutex, MeshSnapshot,
    ServerTls, Snapshot, TOOLS_NOT_FOUND, acquire_instance, child_mode, legacy_program_data_root,
    snapshot_state, stage_wrapper_hashes, start_relay, step_budget,
};
use crate::clock;
use crate::health::{self, HealthGate};
use crate::hosts::{self, HostsControl, HostsManager, HostsTicker, TickerHandle};
use crate::ipc::{Ipc, Mesh};
use crate::maintenance::codec::{self, CodecCapabilities};
use crate::maintenance::patch::{self, PatchConfig, PatchManager};
use crate::maintenance::shutdown::{ShutdownCoordinator, ShutdownStep, panic_text};
use crate::maintenance::{describe_parent_process, update};
use crate::mesh::reporting::Reporter;
use crate::mesh::{Config, MeshClient, connect};
use crate::monitoring::heartbeat::{self, Heartbeat};
use crate::monitoring::vrclog::{MonitorDeps, PlaybackSink, RelayWedge, VrcLogMonitor};
use crate::og_hint::{self, OgFallbackHint};
use crate::relay::server::RelayDeps;
use crate::relay::tls::{self, ChildPaths, RelayTls, WindowsTlsHost};
use crate::relay::upstream::{self, ReqwestUpstream, Upstream};
use crate::relay::{HitchDetector, RelayActivity, RelayBypass, RelayConfig, RelayPortManager, RelayServer};
use crate::resolve_cache::ResolveCache;
use crate::settings::Store;
use crate::stats::Stats;
use crate::terminal::{Services, SessionStore, Terminal, animations_allowed, interactive_allowed};

impl InstanceMutex for NamedMutex {
    fn try_acquire(&mut self) -> bool {
        NamedMutex::try_acquire(self)
    }
}

fn create_mutex(name: &str) -> Result<NamedMutex, CreateError> {
    NamedMutex::create(name).map_err(|e| match e {
        MutexError::AccessDenied => CreateError::AccessDenied,
        MutexError::Other(e) => CreateError::Other(e.to_string()),
    })
}

fn install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn run(build: BuildInfo, args: Vec<String>) -> i32 {
    win::enable_vt_output();
    let paths = AppPaths::from_env();
    paths.migrate_from_legacy_product(&mut |line| println!("{line}"));

    let log = Log::install(&paths.logs_dir(), COMPONENT);
    log.set_dev_console(build.dev);
    log.line(&format!("[console] quick_edit={}", win::disable_quick_edit()));
    let ux = Ux::stdout();
    ux.tee_to(log.clone());

    let reporter = Arc::new(Reporter::from_env(&build.version, ux.clone()));
    let crash = Crash::install(paths.crashes_dir(), COMPONENT, &build.version);

    if let Some(mode) = child_mode(&args) {
        return run_child(mode, &args, &paths, &ux);
    }

    let (instance, local) = acquire_instance(create_mutex);
    if local {
        ux.warn(
            Component::Terminal,
            "could not create global mutex; using session-local mutex.",
        );
    }
    let held = match instance {
        Instance::Acquired(held) => held,
        Instance::AlreadyRunning => {
            ux.warn(Component::Terminal, "VRCResolver is already running.");
            return 1;
        }
        Instance::Failed(e) => {
            eprintln!("Mutex creation failed: {e}");
            return 3;
        }
    };

    let install_dir = install_dir();
    update::apply_staged_updater(&install_dir, &ux, &log);

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            ux.error(
                Component::Terminal,
                &format!("could not start the async runtime: {e}"),
            );
            return 3;
        }
    };
    let code = {
        let _entered = runtime.enter();
        let app = App {
            build,
            paths,
            install_dir,
            ux,
            log,
            reporter,
            crash,
            parts: Arc::new(Parts::default()),
            life: Arc::new(Lifecycle::new(runtime.handle().clone())),
        };
        app.watch(&runtime)
    };
    runtime.shutdown_timeout(Duration::from_millis(500));
    drop(held);
    code
}

fn run_child(mode: ChildMode, args: &[String], paths: &AppPaths, ux: &Ux) -> i32 {
    match mode {
        ChildMode::AddHosts => hosts::run_add(&hosts::hosts_path(), ux),
        ChildMode::RemoveHosts => hosts::run_remove(&hosts::hosts_path(), ux),
        ChildMode::TlsBootstrap | ChildMode::TlsRemove | ChildMode::TlsCheck => {
            let host = match WindowsTlsHost::current() {
                Ok(host) => host,
                Err(e) => {
                    ux.error(Component::Relay, &format!("HTTPS setup failed: {e}"));
                    return 1;
                }
            };
            let program_data_root = paths.program_data_root();
            let child = ChildPaths {
                state_dir: paths.state_root(),
                legacy_program_data_root: legacy_program_data_root(&program_data_root),
                program_data_root,
            };
            match mode {
                ChildMode::TlsBootstrap => {
                    let rest = args.get(1..).unwrap_or(&[]);
                    tls::run_bootstrap(rest, &host, &child, ux, SystemTime::now())
                }
                ChildMode::TlsCheck => tls::run_check(&host, &child),
                _ => tls::run_remove(&host, &child, ux),
            }
        }
    }
}

struct Lifecycle {
    handle: Handle,
    coordinator: OnceLock<ShutdownCoordinator>,
    started: AtomicBool,
    fast: AtomicBool,
    quit: Mutex<bool>,
    wake: Condvar,
}

impl Lifecycle {
    fn new(handle: Handle) -> Lifecycle {
        Lifecycle {
            handle,
            coordinator: OnceLock::new(),
            started: AtomicBool::new(false),
            fast: AtomicBool::new(false),
            quit: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    fn begin(&self, trigger: &str, fast: bool) -> Option<impl Future<Output = ()> + Send + 'static> {
        let coordinator = self.coordinator.get()?;
        if !self.started.swap(true, Ordering::SeqCst) {
            self.fast.store(fast, Ordering::SeqCst);
        }
        let _entered = self.handle.enter();
        Some(coordinator.run(trigger, fast))
    }

    fn shutdown(&self, trigger: &str, fast: bool, limit: Option<Duration>) {
        let Some(done) = self.begin(trigger, fast) else {
            return;
        };
        match limit {
            Some(limit) => {
                let _ = self.handle.block_on(tokio::time::timeout(limit, done));
            }
            None => self.handle.block_on(done),
        }
    }

    fn request_quit(&self) {
        *self.quit.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.notify_all();
    }

    fn wait_quit(&self) {
        let guard = self.quit.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = self
            .wake
            .wait_while(guard, |q| !*q)
            .unwrap_or_else(|e| e.into_inner());
    }

    fn on_ctrl(&self, event: CtrlEvent) {
        let trigger = match event {
            CtrlEvent::CtrlC => {
                drop(self.begin("ctrl-c", false));
                self.request_quit();
                return;
            }
            CtrlEvent::Close => "console-close",
            CtrlEvent::Logoff => "logoff",
            CtrlEvent::Shutdown => "system-shutdown",
        };
        self.shutdown(trigger, true, Some(CLOSE_WAIT));
        self.request_quit();
    }
}

type Task = (CancellationToken, JoinHandle<()>);

#[derive(Default)]
struct Parts {
    terminal: Mutex<Option<Arc<Terminal>>>,
    heartbeat: Mutex<Option<Task>>,
    hosts_ticker: Mutex<Option<TickerHandle>>,
    logmon: Mutex<Option<Task>>,
    ipc: Mutex<Option<Vec<JoinHandle<()>>>>,
    relay: Mutex<Option<Arc<RelayServer>>>,
    mesh: Mutex<Option<MeshClient>>,
    cache: Mutex<Option<Arc<ResolveCache>>>,
    patch: Mutex<Option<Arc<PatchManager>>>,
    ports: Mutex<Option<Arc<RelayPortManager>>>,
}

fn put<T>(slot: &Mutex<Option<T>>, value: T) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
}

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

fn peek<T: Clone>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn present<T>(slot: &Mutex<Option<T>>) -> bool {
    slot.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

async fn blocking(f: impl FnOnce() + Send + 'static) -> Result<(), String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| panic_text(&e))
}

async fn cancel_task((cancel, handle): Task) -> Result<(), String> {
    cancel.cancel();
    handle.await.map_err(|e| panic_text(&e))
}

fn step<F, Fut>(name: &str, run: F) -> ShutdownStep
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    ShutdownStep::new(name, step_budget(name), run)
}

impl Parts {
    fn steps(&self) -> Vec<ShutdownStep> {
        let mut steps = Vec::new();
        if let Some(terminal) = peek(&self.terminal) {
            steps.push(step("terminal", move || blocking(move || terminal.stop())));
        }
        if let Some(task) = take(&self.heartbeat) {
            steps.push(step("heartbeat", move || cancel_task(task)));
        }
        if let Some(ticker) = take(&self.hosts_ticker) {
            steps.push(step("hosts-ticker", move || blocking(move || ticker.stop())));
        }
        if let Some(task) = take(&self.logmon) {
            steps.push(step("logmon", move || cancel_task(task)));
        }
        if let Some(handles) = take(&self.ipc) {
            steps.push(step("ipc", move || async move {
                for handle in handles {
                    handle.abort();
                    let _ = handle.await;
                }
                Ok(())
            }));
        }
        if let Some(relay) = take(&self.relay) {
            let stopping = relay.clone();
            steps.push(step("relay", move || async move {
                stopping.stop().await;
                Ok(())
            }));
            steps.push(step("relay-dispose", move || async move {
                drop(relay);
                Ok(())
            }));
        }
        if let Some(mesh) = peek(&self.mesh) {
            steps.push(step("mesh", move || async move {
                mesh.stop().await;
                Ok(())
            }));
        }
        if let Some(cache) = peek(&self.cache) {
            steps.push(step("resolve-cache", move || blocking(move || cache.flush_now())));
        }
        steps
    }

    fn snapshot(&self, life: &Lifecycle) -> Snapshot {
        Snapshot {
            mesh: peek(&self.mesh).map(|m| MeshSnapshot {
                connected: m.is_connected(),
                server_protocol_version: m.server_protocol_version(),
                node: m.server_node(),
                warp_active: m.warp_active(),
            }),
            patch: peek(&self.patch).map(|p| p.vrc_tools_dir().map(|d| d.display().to_string())),
            ipc: present(&self.ipc),
            terminal: present(&self.terminal),
            shutdown_started: life.started.load(Ordering::SeqCst),
            fast_shutdown: life.fast.load(Ordering::SeqCst),
        }
    }
}

struct App {
    build: BuildInfo,
    paths: AppPaths,
    install_dir: PathBuf,
    ux: Ux,
    log: Log,
    reporter: Arc<Reporter>,
    crash: Crash,
    parts: Arc<Parts>,
    life: Arc<Lifecycle>,
}

impl App {
    fn hook_console(&self) {
        let life = self.life.clone();
        if !win::install_ctrl_handler(Box::new(move |event| life.on_ctrl(event))) {
            self.ux.warn(
                Component::Shutdown,
                "could not hook console close events; if this window is closed, VRChat's tools are put back on the next start.",
            );
        }
        self.log.line(&format!(
            "[startup] parent={}",
            describe_parent_process().unwrap_or_else(|| "unknown".to_owned())
        ));
        let parts = self.parts.clone();
        let life = self.life.clone();
        self.crash
            .set_snapshot(Some(Arc::new(move || snapshot_state(&parts.snapshot(&life)))));
    }

    fn banner(&self, state_root: &Path) {
        let tools = self
            .paths
            .vrc_tools_dir(None)
            .map_or_else(|| TOOLS_NOT_FOUND.to_owned(), |p| p.display().to_string());
        self.ux.banner(&Banner {
            version: &self.build.version,
            sha: &self.build.sha,
            build_time: &self.build.build_time,
            dev: self.build.dev,
            paths: &[
                ("install", self.install_dir.display().to_string()),
                ("vrc tools", tools),
                ("state", state_root.display().to_string()),
                ("os", win::os_description()),
                ("runtime", self.build.runtime.clone()),
            ],
        });
    }

    fn install_shutdown(&self, patcher: Arc<PatchManager>) {
        let parts = self.parts.clone();
        let restore = Arc::new(move |trigger: &str| {
            let restored = patcher.restore_now(trigger);
            if let Some(ports) = peek(&parts.ports) {
                ports.delete_port_file();
            }
            Ok::<bool, String>(restored)
        });
        let parts = self.parts.clone();
        let log = self.log.clone();
        let coordinator = ShutdownCoordinator::new(
            restore,
            Arc::new(move || parts.steps()),
            Arc::new(move |line: &str| log.line(&format!("[shutdown] {line}"))),
        );
        let _ = self.life.coordinator.set(coordinator);
    }

    fn raise_wininet(&self, state_root: &Path) {
        let ux = self.ux.clone();
        let raised = HkcuKey::create(wininet::KEY_PATH).and_then(|key| {
            let mut store = RegDword {
                key: &key,
                name: wininet::VALUE_NAME,
            };
            wininet::ensure(&mut store, &state_root.join(wininet::MARKER_FILE), &mut |msg| {
                ux.write(Component::Relay, &msg)
            })
        });
        if let Err(e) = raised {
            self.ux.warn(
                Component::Relay,
                &format!("could not raise WinINet connection limit: {e}"),
            );
        }
    }

    fn watch(&self, runtime: &Runtime) -> i32 {
        self.hook_console();
        let state_root = self.paths.state_root();
        let settings = Arc::new(Store::new(state_root.join("settings.json"), {
            let ux = self.ux.clone();
            Box::new(move |msg: &str| ux.warn(Component::Terminal, msg))
        }));
        let prefs = settings.snapshot();
        self.banner(&state_root);

        patch::log_vrc_process_state(&self.ux);
        let patcher = Arc::new(PatchManager::new(
            PatchConfig {
                install_dir: self.install_dir.clone(),
                vrc_tools_dir: self.paths.vrc_tools_dir(None),
                state_dir: state_root.clone(),
                tick_delay: patch::DEFAULT_TICK_DELAY,
            },
            self.ux.clone(),
            self.log.clone(),
        ));
        put(&self.parts.patch, patcher.clone());
        self.install_shutdown(patcher.clone());
        patcher.recover_from_unclean_shutdown();

        let started = SystemTime::now();
        let stats = Arc::new(Stats::new(started));
        let mesh = MeshClient::new(
            Config::new(&state_root, &self.build.version),
            self.ux.clone(),
            self.log.clone(),
            stats.clone(),
        );
        put(&self.parts.mesh, mesh.clone());
        let cache = Arc::new(ResolveCache::new(
            state_root.join("resolve_cache.json"),
            clock::system(),
            self.log.clone(),
        ));
        put(&self.parts.cache, cache.clone());
        let og_hint = Arc::new(OgFallbackHint::new(og_hint::DEFAULT_TTL, clock::system()));
        let health = Arc::new(HealthGate::new(
            health::DEFAULT_COOLDOWN,
            health::DEFAULT_VERDICT_TIMEOUT,
            clock::system(),
        ));
        let codecs = Arc::new(CodecCapabilities::system(self.log.clone()));

        let ipc = Ipc::builder(Arc::new(mesh.clone()), self.ux.clone(), self.log.clone())
            .cache(cache.clone())
            .og_hint(og_hint.clone())
            .health(health.clone())
            .stats(stats.clone())
            .high_quality({
                let settings = settings.clone();
                move || settings.snapshot().playback.high_quality
            })
            .verified_codecs({
                let codecs = codecs.clone();
                move || codecs.verified()
            })
            .report_fallback({
                let reporter = self.reporter.clone();
                move |req, reason| reporter.report_fallback(req, reason, None)
            })
            .build();
        put(
            &self.parts.ipc,
            vec![ipc.serve(wire::PIPE_NAME), ipc.serve(legacy::PIPE_NAME)],
        );
        mesh.start();

        self.raise_wininet(&state_root);

        let ports = Arc::new(RelayPortManager::new(&state_root, self.ux.clone()));
        put(&self.parts.ports, ports.clone());
        let activity = Arc::new(RelayActivity::new());
        let relay = if ports.initialize() {
            let https_allowed = prefs.https_enabled();
            if !https_allowed {
                self.ux.write(
                    Component::Relay,
                    "secure local video disabled in settings; using local HTTP fallback.",
                );
            }
            let relay_started = runtime.block_on(self.start_relay(
                &ports,
                https_allowed,
                &state_root,
                mesh.first_party(),
                activity.clone(),
                stats.clone(),
            ));
            if relay_started.is_none() {
                self.ux.warn(
                    Component::Relay,
                    "local video relay could not start -- public-instance local video disabled.",
                );
            }
            relay_started
        } else {
            self.ux.warn(
                Component::Relay,
                "could not reserve a local video port -- public-instance local video disabled.",
            );
            None
        };

        let mut deps = MonitorDeps::new(
            Arc::new(mesh.clone()) as Arc<dyn PlaybackSink>,
            mesh.first_party(),
            self.ux.clone(),
            self.log.clone(),
        );
        deps.cache = Some(cache.clone());
        deps.og_hint = Some(og_hint.clone());
        deps.health = Some(health.clone());
        if let Some((server, scheme)) = relay {
            let bypass = Arc::new(RelayBypass::new(ports.clone(), scheme, self.ux.clone()));
            let engage = bypass.clone();
            deps.relay_wedge = Some(RelayWedge {
                activity: activity.clone(),
                on_wedged: Box::new(move |port| engage.engage(port)),
            });
            deps.on_new_session = Some(Box::new(move || bypass.release()));
            put(&self.parts.relay, Arc::new(server));
        }
        let vrc_dir = self.paths.local_low().join("VRChat").join("VRChat");
        let cancel = CancellationToken::new();
        let handle = VrcLogMonitor::new(deps, vrc_dir).spawn(clock::system(), cancel.clone());
        put(&self.parts.logmon, (cancel, handle));

        if !patcher.start() {
            self.life.shutdown("patch-refused", false, None);
            return 2;
        }

        self.ux.success(
            Component::Patch,
            "VRChat video hook ready; watching for game updates.",
        );
        self.ux.write(
            Component::Terminal,
            "type /help for commands, /status for activity, /settings for options.",
        );
        self.ux.write(
            Component::Terminal,
            "to uninstall, run vrcresolver.Uninstaller.exe from this folder.",
        );

        self.start_hosts();

        let cancel = CancellationToken::new();
        let beat = Arc::new(Heartbeat {
            stats: stats.clone(),
            mesh_connected: Box::new({
                let mesh = mesh.clone();
                move || mesh.is_connected()
            }),
            cache_size: Box::new({
                let cache = cache.clone();
                move || cache.count()
            }),
            now: clock::system(),
            ux: self.ux.clone(),
            log: self.log.clone(),
        });
        put(
            &self.parts.heartbeat,
            (cancel.clone(), beat.spawn(heartbeat::INTERVAL, cancel)),
        );

        let _ = update::start_background_check(
            prefs.maintenance.update_check,
            prefs.maintenance.include_prereleases,
            &self.build.version,
            self.ux.clone(),
            &self.log,
        );
        codecs.refresh();
        let _ = codec::start_background_check(
            prefs.maintenance.codec_auto_install,
            &state_root,
            codecs,
            self.ux.clone(),
            self.log.clone(),
        );
        tools::sweep_legacy_install_tools(&self.install_dir);
        if let Err(e) = stage_wrapper_hashes(&self.install_dir, &state_root) {
            self.log.line(&format!(
                "[startup] could not stage wrapper_hashes for wrapper: {e}"
            ));
        }

        self.start_terminal(settings, stats, mesh, &state_root, started);

        self.life.wait_quit();
        self.ux.write(
            Component::Shutdown,
            "shutting down; restoring VRChat tools and closing local services.",
        );
        self.life.shutdown("quit", false, None);
        0
    }

    async fn start_relay(
        &self,
        ports: &RelayPortManager,
        https_allowed: bool,
        state_root: &Path,
        policy: Arc<std::sync::RwLock<vrcr_shared::first_party::FirstPartyPolicy>>,
        activity: Arc<RelayActivity>,
        stats: Arc<Stats>,
    ) -> Option<(RelayServer, &'static str)> {
        let upstream: Result<Arc<dyn Upstream>, String> = connect::platform_tls()
            .and_then(|tls| ReqwestUpstream::new((*tls).clone()))
            .map(upstream::shared);
        let tls_host = WindowsTlsHost::current().ok();
        let tls_for = |port: u16| -> ServerTls {
            let host = tls_host.as_ref()?;
            RelayTls::new(host, state_root, self.ux.clone(), clock::system()).ensure_ready(port)
        };
        let hitch = Arc::new(HitchDetector::new());
        let start = |port: u16, tls: ServerTls| {
            let upstream = upstream.clone();
            let scheme = if tls.is_some() { "https" } else { "http" };
            let cfg = RelayConfig {
                port,
                scheme: scheme.to_owned(),
                timeout: None,
                tls,
                verbose: self.build.dev,
            };
            let policy = policy.clone();
            let activity = activity.clone();
            let hitch = hitch.clone();
            let stats = stats.clone();
            let ux = self.ux.clone();
            let log = self.log.clone();
            async move {
                let upstream = upstream.map_err(io::Error::other)?;
                let deps = RelayDeps {
                    upstream,
                    policy,
                    activity,
                    hitch,
                    stats,
                    now: clock::system(),
                    ux,
                    log,
                };
                RelayServer::start(cfg, deps).await
            }
        };
        start_relay(ports, https_allowed, tls_for, start, &self.ux).await
    }

    fn start_hosts(&self) {
        let manager = match HostsManager::system(self.ux.clone()) {
            Ok(manager) => Arc::new(manager),
            Err(e) => {
                self.ux
                    .warn(Component::Hosts, &format!("background check failed: {e}"));
                return;
            }
        };
        let ensure = manager.clone();
        let ux = self.ux.clone();
        tokio::spawn(async move {
            if let Err(e) = tokio::task::spawn_blocking(move || ensure.ensure_entry()).await {
                ux.warn(
                    Component::Hosts,
                    &format!("background check failed: {}", panic_text(&e)),
                );
            }
        });
        let ticker = Arc::new(HostsTicker::new(
            manager as Arc<dyn HostsControl>,
            clock::system(),
            self.ux.clone(),
            self.log.clone(),
        ));
        put(&self.parts.hosts_ticker, ticker.start(hosts::TICK_INTERVAL));
    }

    fn start_terminal(
        &self,
        settings: Arc<Store>,
        stats: Arc<Stats>,
        mesh: MeshClient,
        state_root: &Path,
        started: SystemTime,
    ) {
        let stdin_tty = io::stdin().is_terminal();
        let stdout_tty = io::stdout().is_terminal();
        let life = self.life.clone();
        let services = Services {
            ux: self.ux.clone(),
            stats,
            settings,
            mesh_connected: Arc::new(move || mesh.is_connected()),
            request_shutdown: Arc::new(move || life.request_quit()),
            install_dir: self.install_dir.clone(),
            paths: self.paths.clone(),
            spawn_updater: update::spawn_detached,
            width: win::console_width,
            animations_available: animations_allowed(
                stdin_tty,
                self.ux.style().color,
                legacy::env_with_fallback("NO_ANIMATIONS").as_deref(),
            ),
            now: SystemTime::now,
        };
        let session = Arc::new(SessionStore::new(
            &state_root.join("terminal"),
            started,
            SystemTime::now,
        ));
        let terminal = Arc::new(Terminal::new(services, session));
        put(&self.parts.terminal, terminal.clone());
        let allowed = interactive_allowed(
            stdin_tty,
            stdout_tty,
            legacy::env_with_fallback("NO_INTERACTIVE_TERMINAL").as_deref(),
        );
        if allowed {
            terminal.start();
        }
    }
}
