use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use vrcr_shared::console::{Component, Ux};

use super::ports::RelayPortManager;

pub struct RelayBypass {
    ports: Arc<RelayPortManager>,
    scheme: String,
    active: AtomicBool,
    ux: Ux,
}

impl RelayBypass {
    pub fn new(ports: Arc<RelayPortManager>, scheme: &str, ux: Ux) -> RelayBypass {
        RelayBypass {
            ports,
            scheme: scheme.to_owned(),
            active: AtomicBool::new(false),
            ux,
        }
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn engage(&self, wedged_port: u16) -> bool {
        if wedged_port != self.ports.current_port() {
            return self.active();
        }
        if !self.active.swap(true, Ordering::SeqCst) {
            self.ports.delete_port_file();
            self.ux.warn(
                Component::Relay,
                &format!(
                    "VRChat can no longer reach the local video relay on port {wedged_port}; sending direct links until VRChat restarts"
                ),
            );
        }
        true
    }

    pub fn release(&self) {
        if !self.active.swap(false, Ordering::SeqCst) {
            return;
        }
        self.ports.republish(&self.scheme);
        self.ux.write(
            Component::Relay,
            &format!(
                "new VRChat session; local video relay back on port {}",
                self.ports.current_port()
            ),
        );
    }
}
