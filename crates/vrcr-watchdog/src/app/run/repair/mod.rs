use vrcr_shared::console::Color;

use super::*;
use crate::maintenance::SetupError;

pub(super) struct SetupState {
    pass: Mutex<SetupPass>,
    pub(super) relay_https: AtomicBool,
    closing: CancellationToken,
}

impl SetupState {
    pub(super) fn starting(closing: CancellationToken) -> Self {
        Self {
            pass: Mutex::new(SetupPass {
                running: true,
                ..SetupPass::default()
            }),
            relay_https: AtomicBool::new(false),
            closing,
        }
    }

    pub(super) fn pass(&self) -> SetupPass {
        *self.pass.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn start(&self) -> bool {
        let mut pass = self.pass.lock().unwrap_or_else(|error| error.into_inner());
        if pass.running {
            return false;
        }
        pass.running = true;
        true
    }

    pub(super) fn finish(&self, reported: bool) {
        let mut pass = self.pass.lock().unwrap_or_else(|error| error.into_inner());
        *pass = SetupPass {
            generation: pass.generation.wrapping_add(1),
            running: false,
            reported,
        };
    }
}

struct Repair {
    ux: Ux,
    hosts: Arc<HostsManager>,
    ports: Arc<RelayPortManager>,
    state_root: PathBuf,
    state: Arc<SetupState>,
    patcher: Arc<PatchManager>,
}

pub(super) fn command(
    ux: Ux,
    hosts: Arc<HostsManager>,
    ports: Arc<RelayPortManager>,
    state_root: &Path,
    state: Arc<SetupState>,
    patcher: Arc<PatchManager>,
) -> Action {
    let runtime = Handle::current();
    let repair = Arc::new(Repair {
        ux,
        hosts,
        ports,
        state_root: state_root.to_path_buf(),
        state,
        patcher,
    });
    Arc::new(move || repair.start(&runtime))
}

impl Repair {
    fn start(self: &Arc<Self>, runtime: &Handle) {
        if self.state.closing.is_cancelled() {
            return;
        }
        if !self.state.start() {
            self.ux
                .notice(Color::Gray, "Video setup is already running.");
            return;
        }
        let repair = Arc::clone(self);
        let worker = runtime.spawn_blocking(move || repair.run());
        let repair = Arc::clone(self);
        runtime.spawn(async move {
            let reported = match worker.await {
                Ok(reported) => reported,
                Err(error) => {
                    repair
                        .ux
                        .log_error(Component::Terminal, &panic_text(&error));
                    repair.ux.notice(
                        Color::Red,
                        "Couldn't finish the repair. Type repair to be asked again.",
                    );
                    true
                }
            };
            repair.state.finish(reported);
        });
    }

    fn run(&self) -> bool {
        let wrapper = self.patcher.repair_wrapper();
        let (certificate, hosts) = setup::run(
            &self.hosts,
            true,
            &self.state.closing,
            &|changes| permission::ask(changes, &self.state.closing, &self.ux),
            &self.ux,
            |approve| {
                let host = WindowsTlsHost::current().map_err(|error| {
                    self.ux.log_warn(Component::Relay, &error.to_string());
                    SetupError::Failed
                })?;
                RelayTls::new(&host, &self.state_root, self.ux.clone(), clock::system())
                    .with_permission(approve)
                    .repair_trust(self.ports.current_port())
            },
        );
        if !self.state.closing.is_cancelled() {
            report(
                &self.ux,
                hosts,
                certificate,
                wrapper,
                !self.state.relay_https.load(Ordering::Acquire),
            );
        }
        hosts.is_err() || certificate.is_err()
    }
}

fn report(
    ux: &Ux,
    hosts: Result<bool, SetupError>,
    certificate: Result<bool, SetupError>,
    wrapper: Option<bool>,
    needs_restart: bool,
) {
    let mut fixed = Vec::new();
    match hosts {
        Ok(true) => fixed.push("Videos from this PC can now play in public worlds."),
        Ok(false) => {}
        Err(SetupError::Declined) => {
            ux.event(vrcr_shared::console::Notice::HostsPermissionDeclined)
        }
        Err(SetupError::Failed) => ux.event(vrcr_shared::console::Notice::HostsSetupIncomplete),
    }
    match certificate {
        Ok(true) => fixed.push("Secure video is now set up."),
        Ok(false) => {}
        Err(SetupError::Declined) => {
            ux.event(vrcr_shared::console::Notice::SecurePermissionDeclined)
        }
        Err(SetupError::Failed) => ux.event(vrcr_shared::console::Notice::SecureSetupIncomplete),
    }
    match wrapper {
        Some(true) => fixed.push("VRCResolver is connected to VRChat's video player again."),
        Some(false) => {}
        None => ux.notice(
            Color::Yellow,
            "Couldn't reconnect to VRChat's video player. Type repair again after the video stops.",
        ),
    }
    if !fixed.is_empty() {
        ux.notice(Color::White, &fixed.join(" "));
    } else if hosts.is_ok() && certificate.is_ok() && wrapper.is_some() {
        ux.notice(Color::Gray, "There's nothing to repair.");
    }
    if certificate == Ok(true) && needs_restart {
        ux.notice(
            Color::Gray,
            "Restart VRCResolver to use secure video playback.",
        );
    }
}

#[cfg(test)]
mod tests;
