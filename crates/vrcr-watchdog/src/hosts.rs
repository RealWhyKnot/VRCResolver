use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use vrcr_shared::console::{Component, Ux};
pub use vrcr_shared::hosts::{
    ADD_ARG, MARKER_IP, REMOVE_ARG, contains_bypass_entry, hosts_path, line_is_bypass_entry,
};
use vrcr_shared::log::Log;
use vrcr_shared::trust_gateway::GATEWAY_HOST;
use vrcr_shared::win::Elevated;

use crate::clock::Now;

pub const TICK_INTERVAL: Duration = Duration::from_secs(60);
pub const ELEVATION_TIMEOUT: Duration = Duration::from_secs(60);
const READ_ATTEMPTS: u32 = 3;
const READ_RETRY_DELAY: Duration = Duration::from_millis(200);
const DECLINED_REASON: &str = "UAC declined or write blocked";

fn error_text(e: &io::Error) -> String {
    format!("{:?}: {e}", e.kind())
}

pub fn read_bypass_state(path: &Path) -> Result<bool, String> {
    let mut last = String::from("<unknown>");
    for attempt in 1..=READ_ATTEMPTS {
        match fs::read(path) {
            Ok(bytes) => return Ok(contains_bypass_entry(&String::from_utf8_lossy(&bytes))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Err(error_text(&e)),
            Err(e) => last = error_text(&e),
        }
        if attempt < READ_ATTEMPTS {
            thread::sleep(READ_RETRY_DELAY);
        }
    }
    Err(last)
}

pub fn run_add(path: &Path, ux: &Ux) -> i32 {
    if read_bypass_state(path) == Ok(true) {
        return 0;
    }
    let appended = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut f| write!(f, "\r\n{MARKER_IP} {GATEWAY_HOST} # VRCResolver\r\n"));
    match appended {
        Ok(()) => 0,
        Err(e) => {
            ux.error(Component::Hosts, &format!("add failed: {e}"));
            1
        }
    }
}

pub fn run_remove(path: &Path, ux: &Ux) -> i32 {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return 0,
        Err(e) => {
            ux.error(Component::Hosts, &format!("remove failed: {e}"));
            return 1;
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let kept: Vec<&str> = text.lines().filter(|l| !line_is_bypass_entry(l)).collect();
    if kept.len() == text.lines().count() {
        return 0;
    }
    let mut out = String::with_capacity(text.len());
    for line in kept {
        out.push_str(line);
        out.push_str("\r\n");
    }
    match fs::write(path, out) {
        Ok(()) => 0,
        Err(e) => {
            ux.error(Component::Hosts, &format!("remove failed: {e}"));
            1
        }
    }
}

pub type ReadState = Box<dyn Fn() -> Result<bool, String> + Send + Sync>;
pub type Elevate = Box<dyn Fn(&str) -> Elevated + Send + Sync>;

pub struct HostsManager {
    read: ReadState,
    elevate: Elevate,
    ux: Ux,
    in_flight: AtomicBool,
    declined: AtomicBool,
}

impl HostsManager {
    pub fn new(read: ReadState, elevate: Elevate, ux: Ux) -> HostsManager {
        HostsManager {
            read,
            elevate,
            ux,
            in_flight: AtomicBool::new(false),
            declined: AtomicBool::new(false),
        }
    }

    #[cfg(windows)]
    pub fn system(ux: Ux) -> io::Result<HostsManager> {
        let exe = std::env::current_exe()?;
        let path = hosts_path();
        Ok(HostsManager::new(
            Box::new(move || read_bypass_state(&path)),
            Box::new(move |arg| vrcr_shared::win::run_elevated(&exe, arg, ELEVATION_TIMEOUT)),
            ux,
        ))
    }

    pub fn read_state(&self) -> Result<bool, String> {
        (self.read)()
    }

    pub fn is_active(&self) -> bool {
        self.read_state() == Ok(true)
    }

    pub fn prompt_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::Acquire)
    }

    pub fn declined_this_session(&self) -> bool {
        self.declined.load(Ordering::Acquire)
    }

    fn reexec(&self, arg: &str) -> bool {
        match (self.elevate)(arg) {
            Elevated::Exited(_) => true,
            Elevated::TimedOut => {
                self.ux.warn(
                    Component::Hosts,
                    "elevation child still running after 60s -- continuing without hosts entry.",
                );
                true
            }
            Elevated::Declined => {
                self.ux
                    .write(Component::Hosts, "UAC declined -- entry not modified.");
                false
            }
            Elevated::Failed(e) => {
                self.ux.warn(Component::Hosts, &format!("elevation error: {e}"));
                false
            }
        }
    }

    pub fn ensure_entry(&self) {
        if self.is_active() {
            return;
        }
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.ensure_locked();
        self.in_flight.store(false, Ordering::Release);
    }

    fn ensure_locked(&self) {
        if self.is_active() {
            return;
        }
        self.ux.write(
            Component::Hosts,
            "adding entry for public-instance support -- UAC prompt incoming.",
        );
        let launched = self.reexec(ADD_ARG);
        if self.is_active() {
            if launched {
                self.ux
                    .write(Component::Hosts, &format!("added {MARKER_IP} {GATEWAY_HOST}"));
            }
            return;
        }
        self.declined.store(true, Ordering::Release);
        if launched {
            self.ux.warn(
                Component::Hosts,
                "entry not present after elevation -- public-instance support may not work.",
            );
        }
    }

    pub fn remove_entry_if_present(&self) {
        if self.is_active() {
            self.reexec(REMOVE_ARG);
        }
    }
}

pub trait HostsControl: Send + Sync {
    fn read_state(&self) -> Result<bool, String>;
    fn ensure_entry(&self) -> Result<(), String>;
    fn is_active(&self) -> bool;
    fn prompt_in_flight(&self) -> bool;
    fn declined_this_session(&self) -> bool;
}

impl HostsControl for HostsManager {
    fn read_state(&self) -> Result<bool, String> {
        HostsManager::read_state(self)
    }

    fn ensure_entry(&self) -> Result<(), String> {
        HostsManager::ensure_entry(self);
        Ok(())
    }

    fn is_active(&self) -> bool {
        HostsManager::is_active(self)
    }

    fn prompt_in_flight(&self) -> bool {
        HostsManager::prompt_in_flight(self)
    }

    fn declined_this_session(&self) -> bool {
        HostsManager::declined_this_session(self)
    }
}

#[derive(Default)]
struct TickState {
    last_present: Option<bool>,
    gave_up: bool,
}

pub struct HostsTicker {
    hosts: Arc<dyn HostsControl>,
    now: Now,
    ux: Ux,
    log: Log,
    state: Mutex<TickState>,
}

impl HostsTicker {
    pub fn new(hosts: Arc<dyn HostsControl>, now: Now, ux: Ux, log: Log) -> HostsTicker {
        HostsTicker {
            hosts,
            now,
            ux,
            log,
            state: Mutex::new(TickState::default()),
        }
    }

    pub fn tick(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let present = match self.hosts.read_state() {
            Ok(p) => p,
            Err(e) => {
                if st.last_present.is_some() {
                    self.ux.warn(
                        Component::Hosts,
                        &format!("tick: hosts file unreadable ({e}) -- skipping check"),
                    );
                    st.last_present = None;
                }
                return;
            }
        };
        if present {
            if st.last_present != Some(true) {
                self.ux
                    .write(Component::Hosts, &format!("tick: {GATEWAY_HOST} entry present"));
                st.last_present = Some(true);
            }
            return;
        }
        if st.gave_up {
            st.last_present = Some(false);
            return;
        }
        if self.hosts.prompt_in_flight() {
            return;
        }
        if self.hosts.declined_this_session() {
            st.last_present = Some(false);
            self.give_up(&mut st, DECLINED_REASON);
            return;
        }
        if st.last_present != Some(false) {
            self.ux.write(
                Component::Hosts,
                &format!("tick: {GATEWAY_HOST} missing -- re-adding"),
            );
            st.last_present = Some(false);
        }
        let started = (self.now)();
        if let Err(e) = self.hosts.ensure_entry() {
            self.log.line(&format!("[hosts] re-add threw: {e}"));
            self.give_up(&mut st, &e);
            return;
        }
        if self.hosts.is_active() {
            let ms = (self.now)()
                .duration_since(started)
                .unwrap_or_default()
                .as_millis();
            self.ux
                .write(Component::Hosts, &format!("tick: re-add succeeded in {ms} ms"));
            st.last_present = Some(true);
            return;
        }
        if self.hosts.prompt_in_flight() {
            return;
        }
        self.give_up(&mut st, DECLINED_REASON);
    }

    fn give_up(&self, st: &mut TickState, reason: &str) {
        st.gave_up = true;
        self.ux.warn(
            Component::Hosts,
            &format!("tick: re-add failed ({reason}) -- won't ask again until the next start"),
        );
    }

    pub fn start(self: Arc<Self>, interval: Duration) -> TickerHandle {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = thread::Builder::new()
            .name("hosts-ticker".to_owned())
            .spawn(move || {
                loop {
                    self.tick();
                    match stopped.recv_timeout(interval) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .ok();
        TickerHandle {
            stop: Some(stop),
            thread,
        }
    }
}

pub struct TickerHandle {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl TickerHandle {
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for TickerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}
