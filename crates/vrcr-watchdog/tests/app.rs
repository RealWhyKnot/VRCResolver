mod common;

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use tempfile::TempDir;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::{legacy, wire};
use vrcr_watchdog::app::{
    self, BuildInfo, Channel, ChildMode, CreateError, Instance, InstanceMutex, MeshSnapshot, STEP_BUDGETS,
    ServerTls, Snapshot,
};
use vrcr_watchdog::relay::RelayPortManager;
use vrcr_watchdog::relay::ports::{PORT_FILE, SCHEME_FILE};
use vrcr_watchdog::relay::tls;

use common::mesh::Capture;

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (Ux::new(Box::new(out.clone()), Style::plain(), String::new), out)
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn build_info_defaults_match_the_csharp_build_constants() {
    let b = BuildInfo::new("2026.10.14.0", None, Some("  "), None, "rustc 1.99.0");
    assert_eq!(b.version, "2026.10.14.0");
    assert_eq!(b.sha, "<unknown>");
    assert_eq!(b.build_time, "<unknown>");
    assert_eq!(b.channel, Channel::Dev);
    assert!(b.dev);
    assert_eq!(b.channel_label(), Some("dev"));
    assert_eq!(b.runtime, "rustc 1.99.0");

    let release = BuildInfo::new(
        "1.2.3.4",
        Some("abc1234"),
        Some("2026-10-14T08:00:00Z"),
        Some("release"),
        "r",
    );
    assert_eq!(release.sha, "abc1234");
    assert_eq!(release.build_time, "2026-10-14T08:00:00Z");
    assert_eq!(release.channel, Channel::Release);
    assert!(!release.dev);
    assert_eq!(release.channel_label(), None);
}

#[test]
fn nightly_and_unknown_channels_turn_debug_on() {
    let nightly = BuildInfo::new("v", None, None, Some(" Nightly "), "r");
    assert_eq!(nightly.channel, Channel::Nightly);
    assert!(nightly.dev);
    assert_eq!(nightly.channel_label(), Some("nightly"));
    for raw in [None, Some(""), Some("false"), Some("beta")] {
        let b = BuildInfo::new("v", None, None, raw, "r");
        assert_eq!(b.channel, Channel::Dev, "{raw:?}");
        assert!(b.dev, "{raw:?}");
    }
    assert!(!BuildInfo::new("v", None, None, Some("RELEASE"), "r").dev);
}

#[test]
fn elevated_child_flags_dispatch_before_anything_else() {
    assert_eq!(
        app::child_mode(&args(&["--add-hosts-entry"])),
        Some(ChildMode::AddHosts)
    );
    assert_eq!(
        app::child_mode(&args(&["--remove-hosts-entry"])),
        Some(ChildMode::RemoveHosts)
    );
    assert_eq!(
        app::child_mode(&args(&["--local-relay-tls-bootstrap", "51234", "ab"])),
        Some(ChildMode::TlsBootstrap)
    );
    assert_eq!(
        app::child_mode(&args(&["--local-relay-tls-remove"])),
        Some(ChildMode::TlsRemove)
    );
    assert_eq!(
        app::child_mode(&args(&["--local-relay-tls-check"])),
        Some(ChildMode::TlsCheck)
    );
    assert_eq!(app::child_mode(&args(&[])), None);
    assert_eq!(app::child_mode(&args(&["--verbose", "--add-hosts-entry"])), None);
    assert_eq!(app::child_mode(&args(&["--ADD-HOSTS-ENTRY"])), None);
}

#[test]
fn legacy_program_data_sits_beside_the_current_root() {
    let root = Path::new(r"C:\ProgramData\vrcresolver");
    assert_eq!(
        app::legacy_program_data_root(root),
        Path::new(r"C:\ProgramData").join(legacy::STATE_DIR_NAME)
    );
}

#[test]
fn shutdown_budgets_follow_the_csharp_order() {
    let names: Vec<&str> = STEP_BUDGETS.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        vec![
            "terminal",
            "heartbeat",
            "hosts-ticker",
            "logmon",
            "ipc",
            "relay",
            "relay-dispose",
            "mesh",
            "resolve-cache"
        ]
    );
    assert_eq!(app::step_budget("ipc"), Duration::from_millis(3000));
    assert_eq!(app::step_budget("relay"), Duration::from_millis(2000));
    assert_eq!(app::step_budget("logmon"), Duration::from_millis(1000));
    assert_eq!(app::step_budget("unknown"), Duration::from_millis(500));
    let total: u64 = STEP_BUDGETS.iter().map(|(_, ms)| ms).sum();
    assert!(Duration::from_millis(total) <= vrcr_watchdog::maintenance::shutdown::TOTAL_BUDGET);
}

#[derive(Debug)]
struct FakeMutex {
    name: String,
    free: bool,
}

impl InstanceMutex for FakeMutex {
    fn try_acquire(&mut self) -> bool {
        self.free
    }
}

struct World {
    denied: Vec<&'static str>,
    broken: Vec<&'static str>,
    held: Vec<&'static str>,
    created: RefCell<Vec<String>>,
}

impl World {
    fn new() -> World {
        World {
            denied: Vec::new(),
            broken: Vec::new(),
            held: Vec::new(),
            created: RefCell::new(Vec::new()),
        }
    }

    fn create(&self, name: &str) -> Result<FakeMutex, CreateError> {
        self.created.borrow_mut().push(name.to_owned());
        if self.denied.contains(&name) {
            return Err(CreateError::AccessDenied);
        }
        if self.broken.contains(&name) {
            return Err(CreateError::Other(format!("{name} is broken")));
        }
        Ok(FakeMutex {
            name: name.to_owned(),
            free: !self.held.contains(&name),
        })
    }
}

fn held_names(instance: &Instance<FakeMutex>) -> Vec<String> {
    match instance {
        Instance::Acquired(held) => held.iter().map(|m| m.name.clone()).collect(),
        other => panic!("expected the lock, got {other:?}"),
    }
}

#[test]
fn single_instance_takes_both_global_mutexes() {
    let world = World::new();
    let (instance, local) = app::acquire_instance(|n| world.create(n));
    assert!(!local);
    assert_eq!(
        held_names(&instance),
        vec![wire::WATCHDOG_MUTEX.to_owned(), legacy::WATCHDOG_MUTEX.to_owned()]
    );
    assert_eq!(wire::WATCHDOG_MUTEX, "Global\\vrcresolver.Watchdog");
    assert_eq!(wire::WATCHDOG_MUTEX_LOCAL, "Local\\vrcresolver.Watchdog");
}

#[test]
fn access_denied_on_global_falls_back_to_session_local_names() {
    let mut world = World::new();
    world.denied = vec![wire::WATCHDOG_MUTEX];
    let (instance, local) = app::acquire_instance(|n| world.create(n));
    assert!(local);
    assert_eq!(
        held_names(&instance),
        vec![
            wire::WATCHDOG_MUTEX_LOCAL.to_owned(),
            legacy::WATCHDOG_MUTEX_LOCAL.to_owned()
        ]
    );

    let mut world = World::new();
    world.denied = vec![legacy::WATCHDOG_MUTEX];
    let (instance, local) = app::acquire_instance(|n| world.create(n));
    assert!(local);
    assert_eq!(
        held_names(&instance),
        vec![
            wire::WATCHDOG_MUTEX.to_owned(),
            legacy::WATCHDOG_MUTEX_LOCAL.to_owned()
        ]
    );
}

#[test]
fn a_running_watchdog_or_legacy_proxy_blocks_startup() {
    for busy in [wire::WATCHDOG_MUTEX, legacy::WATCHDOG_MUTEX] {
        let mut world = World::new();
        world.held = vec![busy];
        let (instance, _) = app::acquire_instance(|n| world.create(n));
        assert!(matches!(instance, Instance::AlreadyRunning), "{busy}");
        assert_eq!(world.created.borrow().len(), 2);
    }
}

#[test]
fn unexpected_mutex_errors_fail_without_retrying_local() {
    let mut world = World::new();
    world.broken = vec![wire::WATCHDOG_MUTEX];
    let (instance, local) = app::acquire_instance(|n| world.create(n));
    assert!(!local);
    match instance {
        Instance::Failed(e) => assert!(e.contains("broken"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(*world.created.borrow(), vec![wire::WATCHDOG_MUTEX.to_owned()]);
}

fn ports(dir: &TempDir, ux: &Ux) -> RelayPortManager {
    let ports = RelayPortManager::new(dir.path(), ux.clone());
    assert!(ports.initialize());
    ports
}

fn server_tls() -> ServerTls {
    let material = tls::generate(SystemTime::now()).unwrap_or_else(|e| panic!("{e}"));
    Some(tls::server_config(&material).unwrap_or_else(|e| panic!("{e}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Attempt {
    port: u16,
    secure: bool,
}

fn scheme_file(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join(SCHEME_FILE)).unwrap_or_default()
}

#[tokio::test]
async fn relay_starts_https_when_the_cert_is_ready() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = ports(&dir, &ux);
    let port = ports.current_port();
    let started = app::start_relay(
        &ports,
        true,
        |_| server_tls(),
        |port, tls: ServerTls| async move {
            Ok::<_, io::Error>(Attempt {
                port,
                secure: tls.is_some(),
            })
        },
        &ux,
    )
    .await;
    assert_eq!(started, Some((Attempt { port, secure: true }, "https")));
    assert_eq!(scheme_file(&dir), "https");
    assert!(
        out.text().contains(&format!(
            "local video relay ready: https://localhost.youtube.com:{port}"
        )),
        "{}",
        out.text()
    );
}

#[tokio::test]
async fn https_disabled_never_asks_for_a_cert() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, _out) = capture_ux();
    let ports = ports(&dir, &ux);
    let asked = RefCell::new(0);
    let started = app::start_relay(
        &ports,
        false,
        |_| {
            *asked.borrow_mut() += 1;
            server_tls()
        },
        |port, tls: ServerTls| async move {
            Ok::<_, io::Error>(Attempt {
                port,
                secure: tls.is_some(),
            })
        },
        &ux,
    )
    .await;
    assert_eq!(*asked.borrow(), 0);
    assert_eq!(started.map(|(_, s)| s), Some("http"));
    assert_eq!(scheme_file(&dir), "http");
}

#[tokio::test]
async fn failed_https_bind_retries_http_on_the_same_port() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = ports(&dir, &ux);
    let port = ports.current_port();
    let attempts = RefCell::new(Vec::new());
    let started = app::start_relay(
        &ports,
        true,
        |_| server_tls(),
        |port, tls: ServerTls| {
            let secure = tls.is_some();
            attempts.borrow_mut().push(Attempt { port, secure });
            async move {
                if secure {
                    Err(io::Error::other("tls handshake setup failed"))
                } else {
                    Ok(Attempt { port, secure })
                }
            }
        },
        &ux,
    )
    .await;
    assert_eq!(started, Some((Attempt { port, secure: false }, "http")));
    assert_eq!(
        *attempts.borrow(),
        vec![Attempt { port, secure: true }, Attempt { port, secure: false }]
    );
    assert_eq!(scheme_file(&dir), "http");
    let text = out.text();
    assert!(
        text.contains(
            "secure local video failed: tls handshake setup failed -- retrying local HTTP fallback."
        ),
        "{text}"
    );
    assert!(text.contains(&format!(
        "local video relay ready: http://localhost.youtube.com:{port}"
    )));
}

#[tokio::test]
async fn busy_port_moves_to_a_fresh_one() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = ports(&dir, &ux);
    let first = ports.current_port();
    let attempts = RefCell::new(Vec::new());
    let started = app::start_relay(
        &ports,
        false,
        |_| None,
        |port, _tls: ServerTls| {
            attempts.borrow_mut().push(port);
            async move {
                if port == first {
                    Err(io::Error::new(io::ErrorKind::AddrInUse, "address in use"))
                } else {
                    Ok(port)
                }
            }
        },
        &ux,
    )
    .await;
    let second = ports.current_port();
    assert_ne!(first, second);
    assert_eq!(started, Some((second, "http")));
    assert_eq!(*attempts.borrow(), vec![first, second]);
    let text = out.text();
    assert!(
        text.contains(&format!(
            "local video relay could not start on port {first}: address in use"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("(previous port {first} failed: address in use)")),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(PORT_FILE)).unwrap_or_default(),
        second.to_string()
    );
}

#[tokio::test]
async fn relay_that_never_starts_unpublishes_the_port() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, _out) = capture_ux();
    let ports = ports(&dir, &ux);
    assert!(dir.path().join(PORT_FILE).exists());
    let calls = RefCell::new(0);
    let started: Option<(u16, &str)> = app::start_relay(
        &ports,
        true,
        |_| server_tls(),
        |_port, _tls: ServerTls| {
            *calls.borrow_mut() += 1;
            async { Err(io::Error::other("nope")) }
        },
        &ux,
    )
    .await;
    assert!(started.is_none());
    assert_eq!(*calls.borrow(), 4);
    assert!(!dir.path().join(PORT_FILE).exists());
    assert!(!dir.path().join(SCHEME_FILE).exists());
}

#[test]
fn crash_snapshot_matches_the_csharp_layout() {
    let empty = app::snapshot_state(&Snapshot::default());
    assert_eq!(
        empty,
        "mesh:    <not constructed>\npatch:   <not constructed>\nipc:     <not constructed>\nterminal: <not constructed>\nshutdown_started=0 fast_shutdown=False\n"
    );
    let full = app::snapshot_state(&Snapshot {
        mesh: Some(MeshSnapshot {
            connected: true,
            server_protocol_version: 3,
            node: Some("node1".to_owned()),
            warp_active: None,
        }),
        patch: Some(None),
        ipc: true,
        terminal: true,
        shutdown_started: true,
        fast_shutdown: true,
    });
    assert_eq!(
        full,
        "mesh:    connected=True server_protocol_version=3 node=node1 warp_active=?\npatch:   vrcToolsDir=<null>\nipc:     <running>\nterminal: <running>\nshutdown_started=1 fast_shutdown=True\n"
    );
}

#[test]
fn wrapper_hashes_are_staged_into_the_state_root() {
    let install = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let state = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let root = state.path().join("vrcresolver");
    assert!(!app::stage_wrapper_hashes(install.path(), &root).unwrap_or_else(|e| panic!("{e}")));
    assert!(!root.exists());

    fs::create_dir_all(install.path().join("data")).unwrap_or_else(|e| panic!("{e}"));
    fs::write(install.path().join("data").join(app::WRAPPER_HASHES), "abc\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(app::stage_wrapper_hashes(install.path(), &root).unwrap_or_else(|e| panic!("{e}")));
    assert_eq!(
        fs::read_to_string(root.join(app::WRAPPER_HASHES)).unwrap_or_default(),
        "abc\n"
    );
}
