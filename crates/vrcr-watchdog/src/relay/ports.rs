use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};

use vrcr_shared::console::{Component, Ux};
use vrcr_shared::trust_gateway;

pub const PORT_FILE: &str = "relay_port.txt";
pub const LAST_PORT_FILE: &str = "relay_last_port.txt";
pub const SCHEME_FILE: &str = "relay_scheme.txt";

pub struct RelayPortManager {
    state_root: PathBuf,
    current: AtomicU16,
    ux: Ux,
}

impl RelayPortManager {
    pub fn new(state_root: &Path, ux: Ux) -> RelayPortManager {
        RelayPortManager {
            state_root: state_root.to_path_buf(),
            current: AtomicU16::new(0),
            ux,
        }
    }

    pub fn current_port(&self) -> u16 {
        self.current.load(Ordering::SeqCst)
    }

    pub fn initialize(&self) -> bool {
        if let Err(e) = fs::create_dir_all(&self.state_root) {
            self.ux.warn(
                Component::Relay,
                &format!("could not create {}: {e}", self.state_root.display()),
            );
        }
        let previous = self.read_previous_port();
        if let Some(prev) = previous.filter(|p| claim_specific(*p)) {
            self.publish(prev);
            self.ux.write(
                Component::Relay,
                &format!("reserved local video port {prev} (reused)"),
            );
            return true;
        }
        let Some(fresh) = claim_ephemeral() else {
            self.ux
                .error(Component::Relay, "could not reserve a local video port");
            return false;
        };
        self.publish(fresh);
        let suffix = if previous.is_some() {
            " (previous port was busy)"
        } else {
            ""
        };
        self.ux.write(
            Component::Relay,
            &format!("reserved local video port {fresh}{suffix}"),
        );
        true
    }

    pub fn try_reserve_fresh_port(&self, reason: &str) -> bool {
        let Some(fresh) = claim_ephemeral() else {
            self.ux.error(
                Component::Relay,
                "could not reserve a replacement local video port",
            );
            return false;
        };
        let previous = self.current_port();
        self.publish(fresh);
        let detail = if previous > 0 {
            let why = if reason.trim().is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            };
            format!(" (previous port {previous} failed{why})")
        } else {
            String::new()
        };
        self.ux.write(
            Component::Relay,
            &format!("reserved local video port {fresh}{detail}"),
        );
        true
    }

    pub fn republish(&self, scheme: &str) {
        self.write_atomic(PORT_FILE, &self.current_port().to_string());
        self.write_scheme_file(scheme);
    }

    pub fn write_scheme_file(&self, scheme: &str) {
        let scheme = if trust_gateway::is_allowed_scheme(scheme) {
            scheme.to_ascii_lowercase()
        } else {
            "http".to_owned()
        };
        self.write_atomic(SCHEME_FILE, &scheme);
    }

    pub fn delete_port_file(&self) {
        let _ = fs::remove_file(self.state_root.join(PORT_FILE));
        let _ = fs::remove_file(self.state_root.join(SCHEME_FILE));
    }

    fn publish(&self, port: u16) {
        self.current.store(port, Ordering::SeqCst);
        self.write_atomic(PORT_FILE, &port.to_string());
        self.write_atomic(LAST_PORT_FILE, &port.to_string());
    }

    fn read_previous_port(&self) -> Option<u16> {
        let text = fs::read_to_string(self.state_root.join(LAST_PORT_FILE)).ok()?;
        text.trim().parse::<u16>().ok().filter(|p| *p > 1024)
    }

    fn write_atomic(&self, name: &str, content: &str) {
        let path = self.state_root.join(name);
        let tmp = self.state_root.join(format!("{name}.tmp"));
        let result = fs::write(&tmp, content).and_then(|()| fs::rename(&tmp, &path));
        if let Err(e) = result {
            self.ux
                .warn(Component::Relay, &format!("could not write {name}: {e}"));
        }
    }
}

fn claim_specific(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

fn claim_ephemeral() -> Option<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    (port > 0).then_some(port)
}
