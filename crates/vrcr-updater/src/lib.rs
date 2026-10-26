use std::time::{Duration, Instant};

pub mod archive;
pub mod flow;
pub mod install;
pub mod release;
pub mod temp;

pub use vrcr_shared::install_manifest::MANIFEST_REL;

pub const WATCHDOG_EXE: &str = "vrcresolver.exe";
pub const LEGACY_WATCHDOG_EXE: &str = "WKVRCProxy.exe";
pub const WATCHDOG_PROCESSES: [&str; 2] = ["vrcresolver", "WKVRCProxy"];
pub const UPDATER_EXE: &str = "vrcresolver.Updater.exe";
pub const STAGED_UPDATER_EXE: &str = "vrcresolver.Updater.next.exe";
pub const ASSUME_YES_SUFFIX: &str = "UPDATE_REQUESTED";
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(15);
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
pub const DOWNLOAD_CAP: Duration = Duration::from_secs(600);
pub const RELEASE_BUDGET: Duration = Duration::from_secs(5);
pub const RELEASE_POLL: Duration = Duration::from_millis(100);
pub const USER_AGENT: &str = "VRCResolver-Updater/1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptPolicy {
    Requested,
    Redirected,
    Ask,
}

pub fn prompt_policy(assume_yes: Option<&str>, stdin_redirected: bool) -> PromptPolicy {
    if assume_yes == Some("1") {
        PromptPolicy::Requested
    } else if stdin_redirected {
        PromptPolicy::Redirected
    } else {
        PromptPolicy::Ask
    }
}

pub fn wait_for_release(
    any_left: &dyn Fn() -> bool,
    mutexes_free: &dyn Fn() -> bool,
    sleep: &dyn Fn(Duration),
    now: &dyn Fn() -> Instant,
) -> Result<(), Duration> {
    let start = now();
    while now().duration_since(start) < RELEASE_BUDGET {
        if !any_left() && mutexes_free() {
            return Ok(());
        }
        sleep(RELEASE_POLL);
    }
    Err(now().duration_since(start))
}
