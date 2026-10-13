use std::fmt::Write as _;
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use vrcr_shared::console::{Component, Ux};
use vrcr_shared::{legacy, wire};

use crate::hosts;
use crate::relay::ports::RelayPortManager;
use crate::relay::tls;

#[cfg(windows)]
mod run;
#[cfg(windows)]
pub use run::run;

pub const COMPONENT: &str = "watchdog";
pub const TOOLS_NOT_FOUND: &str = "<not found \u{2014} launch VRChat once>";
pub const CLOSE_WAIT: Duration = Duration::from_millis(4500);
pub const WRAPPER_HASHES: &str = "wrapper_hashes.txt";
pub const RELAY_HOST: &str = "localhost.youtube.com";
pub const STEP_BUDGETS: [(&str, u64); 9] = [
    ("terminal", 500),
    ("heartbeat", 500),
    ("hosts-ticker", 500),
    ("logmon", 1000),
    ("ipc", 3000),
    ("relay", 2000),
    ("relay-dispose", 500),
    ("mesh", 3000),
    ("resolve-cache", 1000),
];

pub fn step_budget(name: &str) -> Duration {
    STEP_BUDGETS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(Duration::from_millis(500), |(_, ms)| Duration::from_millis(*ms))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub sha: String,
    pub build_time: String,
    pub dev: bool,
    pub runtime: String,
}

impl BuildInfo {
    pub fn new(
        version: &str,
        sha: Option<&str>,
        build_time: Option<&str>,
        dev: Option<&str>,
        runtime: &str,
    ) -> BuildInfo {
        let known = |v: Option<&str>| {
            v.map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or("<unknown>")
                .to_owned()
        };
        BuildInfo {
            version: version.to_owned(),
            sha: known(sha),
            build_time: known(build_time),
            dev: !dev
                .map(str::trim)
                .is_some_and(|v| v == "0" || v.eq_ignore_ascii_case("false")),
            runtime: runtime.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildMode {
    AddHosts,
    RemoveHosts,
    TlsBootstrap,
    TlsRemove,
    TlsCheck,
}

pub fn child_mode(args: &[String]) -> Option<ChildMode> {
    match args.first()?.as_str() {
        hosts::ADD_ARG => Some(ChildMode::AddHosts),
        hosts::REMOVE_ARG => Some(ChildMode::RemoveHosts),
        tls::BOOTSTRAP_ARG => Some(ChildMode::TlsBootstrap),
        tls::REMOVE_ARG => Some(ChildMode::TlsRemove),
        tls::CHECK_ARG => Some(ChildMode::TlsCheck),
        _ => None,
    }
}

pub fn legacy_program_data_root(program_data_root: &Path) -> PathBuf {
    program_data_root.with_file_name(legacy::STATE_DIR_NAME)
}

pub trait InstanceMutex {
    fn try_acquire(&mut self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    AccessDenied,
    Other(String),
}

#[derive(Debug)]
pub enum Instance<M> {
    Acquired(Vec<M>),
    AlreadyRunning,
    Failed(String),
}

fn describe(e: CreateError) -> String {
    match e {
        CreateError::AccessDenied => "access denied".to_owned(),
        CreateError::Other(m) => m,
    }
}

fn create_pair<M>(create: &impl Fn(&str) -> Result<M, CreateError>) -> Result<(M, M, bool), String> {
    let primary = match create(wire::WATCHDOG_MUTEX) {
        Ok(m) => m,
        Err(CreateError::AccessDenied) => {
            let primary = create(wire::WATCHDOG_MUTEX_LOCAL).map_err(describe)?;
            let legacy = create(legacy::WATCHDOG_MUTEX_LOCAL).map_err(describe)?;
            return Ok((primary, legacy, true));
        }
        Err(CreateError::Other(e)) => return Err(e),
    };
    match create(legacy::WATCHDOG_MUTEX) {
        Ok(legacy) => Ok((primary, legacy, false)),
        Err(CreateError::AccessDenied) => {
            let legacy = create(legacy::WATCHDOG_MUTEX_LOCAL).map_err(describe)?;
            Ok((primary, legacy, true))
        }
        Err(CreateError::Other(e)) => Err(e),
    }
}

pub fn acquire_instance<M: InstanceMutex>(
    create: impl Fn(&str) -> Result<M, CreateError>,
) -> (Instance<M>, bool) {
    let (mut primary, mut legacy, local) = match create_pair(&create) {
        Ok(created) => created,
        Err(e) => return (Instance::Failed(e), false),
    };
    let acquired = primary.try_acquire();
    let legacy_acquired = legacy.try_acquire();
    if acquired && legacy_acquired {
        (Instance::Acquired(vec![primary, legacy]), local)
    } else {
        (Instance::AlreadyRunning, local)
    }
}

pub type ServerTls = Option<Arc<rustls::ServerConfig>>;

async fn start_on_current_port<T, S, Fut>(
    ports: &RelayPortManager,
    https_allowed: bool,
    tls_for: &impl Fn(u16) -> ServerTls,
    start: &S,
    ux: &Ux,
) -> Result<(T, &'static str), String>
where
    S: Fn(u16, ServerTls) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let port = ports.current_port();
    let tls = if https_allowed { tls_for(port) } else { None };
    let secure = tls.is_some();
    let scheme = if secure { "https" } else { "http" };
    ports.write_scheme_file(scheme);
    let ready = |scheme: &str| {
        ux.success(
            Component::Relay,
            &format!("local video relay ready: {scheme}://{RELAY_HOST}:{port}"),
        );
    };
    let could_not_start = |e: &io::Error| {
        ux.warn(
            Component::Relay,
            &format!("local video relay could not start on port {port}: {e}"),
        );
        e.to_string()
    };
    match start(port, tls).await {
        Ok(server) => {
            ready(scheme);
            Ok((server, scheme))
        }
        Err(e) if !secure => Err(could_not_start(&e)),
        Err(e) => {
            ux.warn(
                Component::Relay,
                &format!("secure local video failed: {e} -- retrying local HTTP fallback."),
            );
            ports.write_scheme_file("http");
            match start(port, None).await {
                Ok(server) => {
                    ready("http");
                    Ok((server, "http"))
                }
                Err(e) => Err(could_not_start(&e)),
            }
        }
    }
}

pub async fn start_relay<T, S, Fut>(
    ports: &RelayPortManager,
    https_allowed: bool,
    tls_for: impl Fn(u16) -> ServerTls,
    start: S,
    ux: &Ux,
) -> Option<(T, &'static str)>
where
    S: Fn(u16, ServerTls) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let failure = match start_on_current_port(ports, https_allowed, &tls_for, &start, ux).await {
        Ok(started) => return Some(started),
        Err(failure) => failure,
    };
    if ports.try_reserve_fresh_port(&failure)
        && let Ok(started) = start_on_current_port(ports, https_allowed, &tls_for, &start, ux).await
    {
        return Some(started);
    }
    ports.delete_port_file();
    None
}

pub fn stage_wrapper_hashes(install_dir: &Path, state_root: &Path) -> io::Result<bool> {
    let src = install_dir.join("data").join(WRAPPER_HASHES);
    if !src.is_file() {
        return Ok(false);
    }
    fs::create_dir_all(state_root)?;
    fs::copy(&src, state_root.join(WRAPPER_HASHES))?;
    Ok(true)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeshSnapshot {
    pub connected: bool,
    pub server_protocol_version: i32,
    pub node: Option<String>,
    pub warp_active: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub mesh: Option<MeshSnapshot>,
    pub patch: Option<Option<String>>,
    pub ipc: bool,
    pub terminal: bool,
    pub shutdown_started: bool,
    pub fast_shutdown: bool,
}

fn cs_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

pub fn snapshot_state(s: &Snapshot) -> String {
    let mut out = String::new();
    match &s.mesh {
        Some(m) => {
            let _ = writeln!(
                out,
                "mesh:    connected={} server_protocol_version={} node={} warp_active={}",
                cs_bool(m.connected),
                m.server_protocol_version,
                m.node.as_deref().unwrap_or("?"),
                m.warp_active.map_or("?", cs_bool)
            );
        }
        None => out.push_str("mesh:    <not constructed>\n"),
    }
    match &s.patch {
        Some(dir) => {
            let _ = writeln!(out, "patch:   vrcToolsDir={}", dir.as_deref().unwrap_or("<null>"));
        }
        None => out.push_str("patch:   <not constructed>\n"),
    }
    let state = |on: bool| if on { "<running>" } else { "<not constructed>" };
    let _ = writeln!(out, "ipc:     {}", state(s.ipc));
    let _ = writeln!(out, "terminal: {}", state(s.terminal));
    let _ = writeln!(
        out,
        "shutdown_started={} fast_shutdown={}",
        u8::from(s.shutdown_started),
        cs_bool(s.fast_shutdown)
    );
    out
}
