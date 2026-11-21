use super::*;
use crate::monitoring::vrchat::Session;

pub const VRCHAT_EXIT: &str = "shutdown-vrchat_exit";

impl PatchManager {
    pub fn pause_for_vrchat_exit(&self) -> bool {
        let mut st = self.lock();
        if st.restored || st.paused {
            return false;
        }
        let Some(dir) = self.tools_dir.as_deref() else {
            return false;
        };
        let (outcome, detail) = tools::restore(dir, None, &RealOps);
        if outcome == RestoreOutcome::Failed {
            self.log.line(&format!(
                "[patch] restore (vrchat-exit): {outcome:?} {detail}"
            ));
            return false;
        }
        tools::sweep(dir, &self.log, &self.alive);
        st.paused = true;
        st.previous = None;
        st.incomplete_reported = None;
        st.last_outcome = TickOutcome::None;
        st.lock_failures = 0;
        let flag = self.write_clean_exit_flag();
        self.log.line(&format!(
            "[patch] restore (vrchat-exit): {outcome:?} {detail} flag={flag}"
        ));
        drop(st);
        true
    }

    pub fn resume_after_vrchat_launch(&self) -> bool {
        let mut st = self.lock();
        if st.restored || !st.paused {
            return false;
        }
        st.paused = false;
        if let Err(error) = fs::remove_file(&self.clean_exit_flag)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            self.ux.log_warn(
                Component::Patch,
                &format!("could not remove clean_exit.flag: {error}"),
            );
        }
        self.log
            .line("[patch] VRChat opened again; patching resumes");
        self.tick_core(&mut st, false);
        drop(st);
        true
    }

    pub fn paused_for_vrchat(&self) -> bool {
        self.lock().paused
    }
}

pub struct PatchSession {
    patcher: Arc<PatchManager>,
    state_dir: PathBuf,
    ux: Ux,
}

impl PatchSession {
    pub fn new(patcher: Arc<PatchManager>, state_dir: &Path, ux: Ux) -> PatchSession {
        PatchSession {
            patcher,
            state_dir: state_dir.to_path_buf(),
            ux,
        }
    }
}

impl Session for PatchSession {
    fn exited(&self) -> bool {
        if !self.patcher.pause_for_vrchat_exit() {
            return false;
        }
        crate::diagnostics::startup::record(&self.state_dir, VRCHAT_EXIT);
        self.ux.log_write(
            Component::Patch,
            "VRChat closed; its own yt-dlp is back in place until it opens again.",
        );
        true
    }

    fn gave_up(&self, tries: u32) {
        self.ux.log_warn(
            Component::Patch,
            &format!(
                "VRChat closed, but its own yt-dlp.exe could not be put back after {tries} tries; \
                 it will be tried again when VRChat next opens and closes."
            ),
        );
    }

    fn launched(&self) {
        if !self.patcher.resume_after_vrchat_launch() {
            return;
        }
        crate::diagnostics::startup::record(&self.state_dir, "running");
        self.ux.log_write(
            Component::Patch,
            "VRChat opened; reconnecting its video player.",
        );
    }
}
