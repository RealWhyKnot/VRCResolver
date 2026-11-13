use std::future::Future;
use std::io::Write as _;
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
    BuildInfo, CLOSE_WAIT, COMPONENT, ChildMode, CreateError, Instance, InstanceMutex,
    MeshSnapshot, ServerTls, Snapshot, TOOLS_NOT_FOUND, acquire_instance, child_mode,
    legacy_program_data_root, snapshot_state, stage_wrapper_hashes, start_relay, step_budget,
};
use crate::clock;
use crate::health::{self, HealthGate};
use crate::hosts::{self, HostsManager, HostsTicker, TickerHandle};
use crate::ipc::{Ipc, Mesh};
use crate::local::store::Manifests;
use crate::local::{self, Local, Origin, Step};
use crate::maintenance::codec::{self, CodecCapabilities};
use crate::maintenance::patch::{self, PatchConfig, PatchManager};
use crate::maintenance::shutdown::{ShutdownCoordinator, ShutdownStep, cancel_all, panic_text};
use crate::maintenance::{describe_parent_process, update};
use crate::mesh::reporting::Reporter;
use crate::mesh::{Config, MeshClient, connect};
use crate::monitoring::heartbeat::{self, Heartbeat};
use crate::monitoring::perf::{self, PerfReporter, Phases, SystemInfo};
use crate::monitoring::vrclog::{MonitorDeps, RelayWedge, VrcLogMonitor};
use crate::og_hint::{self, OgFallbackHint};
use crate::relay::server::RelayDeps;
use crate::relay::tls::{self, ChildPaths, RelayTls, WindowsTlsHost};
use crate::relay::upstream::{self, ReqwestUpstream, Upstream};
use crate::relay::{
    HitchDetector, RelayActivity, RelayBypass, RelayConfig, RelayPortManager, RelayServer,
};
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

pub fn run(build: BuildInfo, args: &[String]) -> i32 {
    win::enable_vt_output();
    let paths = AppPaths::from_env();
    paths.migrate_from_legacy_product(&mut |line| {
        let _ = writeln!(std::io::stdout(), "{line}");
    });

    let log = Log::install(&paths.logs_dir(), COMPONENT);
    log.set_dev_console(build.dev);
    log.line(&format!(
        "[console] quick_edit={}",
        win::disable_quick_edit()
    ));
    let ux = Ux::new(
        Box::new(io::stdout()),
        crate::diagnostics::startup::console_style(
            !io::stdout().is_terminal(),
            std::env::var("NO_COLOR").ok().as_deref(),
        ),
        vrcr_shared::console::local_clock,
    )
    .with_width(win::console_width);
    ux.tee_to(log.clone());

    let reporter = Arc::new(Reporter::from_env(&build.version, ux.clone()));
    let crash = Crash::install(paths.crashes_dir(), COMPONENT, &build.version);

    if let Some(mode) = child_mode(args) {
        return run_child(mode, args, &paths, &ux);
    }

    let (instance, local) = acquire_instance(create_mutex);
    if local {
        ux.log_warn(
            Component::Terminal,
            "could not create global mutex; using session-local mutex.",
        );
    }
    let held = match instance {
        Instance::Acquired(held) => held,
        Instance::AlreadyRunning => {
            ux.event(vrcr_shared::console::Notice::AlreadyRunning);
            ux.log_warn(Component::Terminal, "VRCResolver is already running.");
            return 1;
        }
        Instance::Failed(e) => {
            log.line(&format!("Mutex creation failed: {e}"));
            ux.event(vrcr_shared::console::Notice::InstanceCheckFailed);
            return 3;
        }
    };

    let install_dir = install_dir();
    update::apply_staged_updater(&install_dir, &ux, &log);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            ux.event(vrcr_shared::console::Notice::StartupFailed);
            ux.log_error(
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
                    ux.event(vrcr_shared::console::Notice::SecureSetupFailed);
                    ux.log_error(Component::Relay, &format!("HTTPS setup failed: {e}"));
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

    fn begin(
        &self,
        trigger: &str,
        fast: bool,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
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
    perf: Mutex<Option<Task>>,
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
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| panic_text(&e))
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
        let beats: Vec<Task> = take(&self.perf)
            .into_iter()
            .chain(take(&self.heartbeat))
            .collect();
        if !beats.is_empty() {
            steps.push(step("heartbeat", move || cancel_all(beats)));
        }
        if let Some(ticker) = take(&self.hosts_ticker) {
            steps.push(step("hosts-ticker", move || {
                blocking(move || ticker.stop())
            }));
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
            let stopping = std::sync::Arc::clone(&relay);
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
            steps.push(step("resolve-cache", move || {
                blocking(move || cache.flush_now())
            }));
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
        let life = std::sync::Arc::clone(&self.life);
        if !win::install_ctrl_handler(Box::new(move |event| life.on_ctrl(event))) {
            self.ux
                .event(vrcr_shared::console::Notice::CloseHandlerFailed);
            self.ux.log_warn(
                Component::Shutdown,
                "could not hook console close events; if this window is closed, VRChat's tools are put back on the next start.",
            );
        }
        self.log.line(&format!(
            "[startup] parent={}",
            describe_parent_process().unwrap_or_else(|| "unknown".to_owned())
        ));
        let parts = std::sync::Arc::clone(&self.parts);
        let life = std::sync::Arc::clone(&self.life);
        let state_root = self.paths.state_root();
        self.crash.set_snapshot(Some(Arc::new(move || {
            crate::diagnostics::startup::record(&state_root, "panic_observed");
            snapshot_state(&parts.snapshot(&life))
        })));
    }

    fn banner(&self, state_root: &Path) {
        let tools = self
            .paths
            .vrc_tools_dir(None)
            .map_or_else(|| TOOLS_NOT_FOUND.to_owned(), |p| p.display().to_string());
        self.ux.banner(&Banner {
            version: &self.build.version,
            channel: self.build.channel_label(),
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
        let parts = std::sync::Arc::clone(&self.parts);
        let ux = self.ux.clone();
        let restore = Arc::new(move |trigger: &str| {
            let restored = patcher.restore_now(trigger);
            if let Some(ports) = peek(&parts.ports) {
                ports.delete_port_file();
            }
            if restored {
                ux.event(vrcr_shared::console::Notice::Closing);
            } else {
                ux.event(vrcr_shared::console::Notice::RestoreFailed);
            }
            Ok::<bool, String>(restored)
        });
        let parts = std::sync::Arc::clone(&self.parts);
        let log = self.log.clone();
        let state_root = self.paths.state_root();
        let coordinator = ShutdownCoordinator::new(
            restore,
            Arc::new(move || parts.steps()),
            Arc::new(move |line: &str| {
                if let Some(trigger) = line.strip_prefix("shutdown trigger=") {
                    crate::diagnostics::startup::record(
                        &state_root,
                        &format!("shutdown-{}", crate::diagnostics::token(trigger)),
                    );
                }
                log.line(&format!("[shutdown] {line}"));
            }),
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
            wininet::ensure(
                &mut store,
                &state_root.join(wininet::MARKER_FILE),
                &mut |msg| ux.log_write(Component::Relay, &msg),
            )
        });
        if let Err(e) = raised {
            self.ux.log_warn(
                Component::Relay,
                &format!("could not raise WinINet connection limit: {e}"),
            );
        }
    }

    fn start_hosts(&self) {
        let manager = match HostsManager::system(self.ux.clone()) {
            Ok(manager) => Arc::new(manager),
            Err(e) => {
                self.ux
                    .log_warn(Component::Hosts, &format!("background check failed: {e}"));
                return;
            }
        };
        let ensure = std::sync::Arc::clone(&manager);
        let ux = self.ux.clone();
        tokio::spawn(async move {
            if let Err(e) = tokio::task::spawn_blocking(move || ensure.ensure_entry()).await {
                ux.log_warn(
                    Component::Hosts,
                    &format!("background check failed: {}", panic_text(&e)),
                );
            }
        });
        let ticker = Arc::new(HostsTicker::new(
            manager,
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
        let life = std::sync::Arc::clone(&self.life);
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
                crate::terminal::interactive::OutputSupport {
                    input_is_terminal: stdin_tty,
                    output_supported: self.ux.style().color,
                },
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
        put(&self.parts.terminal, std::sync::Arc::clone(&terminal));
        let allowed = interactive_allowed(
            crate::terminal::interactive::OutputSupport {
                input_is_terminal: stdin_tty,
                output_supported: stdout_tty,
            },
            legacy::env_with_fallback("NO_INTERACTIVE_TERMINAL").as_deref(),
        );
        if allowed {
            terminal.start();
        }
    }
}

mod startup;
