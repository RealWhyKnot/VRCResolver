use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::release::pick_highest_entry;

use super::update_dialog::{is_allowed_release_url, release_tag_url, releases_page_url};

pub use super::update_prompt::{
    CheckFlight, CheckStart, CheckTicket, Prompt, Prompter, check_and_offer,
    check_and_offer_manual, normalize_version,
};

pub use vrcr_shared::release::{ANY_RELEASES_URL, REPO, STABLE_LATEST_URL, Version};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
pub const UPDATE_REQUESTED_ENV: &str = "VRCRESOLVER_UPDATE_REQUESTED";
pub const UPDATER_EXE: &str = "vrcresolver.Updater.exe";
pub const STAGED_UPDATER_PAIRS: [(&str, &str); 1] = [("vrcresolver.Updater.next.exe", UPDATER_EXE)];
const MOVE_RETRIES: u32 = 3;
const MOVE_RETRY_DELAY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub html_url: String,
    pub body: Option<String>,
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    UpdateAvailable(Release),
    UpToDate,
    Failed(String),
}

fn release_of(entry: &Value) -> Option<Release> {
    let obj = entry.as_object()?;
    let text = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let body = text("body").trim().to_owned();
    Some(Release {
        tag: text("tag_name"),
        html_url: text("html_url"),
        body: (!body.is_empty()).then_some(body),
        prerelease: obj.get("prerelease") == Some(&Value::Bool(true)),
    })
}

pub fn pick_single(doc: &Value) -> Option<Release> {
    release_of(doc)
}

pub fn pick_highest(list: &Value) -> Option<Release> {
    pick_highest_entry(list).and_then(release_of)
}

pub fn available_release(doc: &Value, include_prereleases: bool, local: &str) -> Option<Release> {
    let best = if include_prereleases {
        pick_highest(doc)
    } else {
        pick_single(doc)
    }?;
    if best.tag.is_empty() {
        return None;
    }
    let remote = Version::from_tag(&best.tag)?;
    let local = Version::from_tag(local)?;
    if remote <= local {
        return None;
    }
    Some(best)
}

pub fn available_message(doc: &Value, include_prereleases: bool, local: &str) -> Option<String> {
    available_release(doc, include_prereleases, local).map(|release| release.log_message())
}

impl Release {
    pub fn notes_url(&self) -> String {
        if is_allowed_release_url(&self.html_url) {
            return self.html_url.clone();
        }
        release_tag_url(&self.tag).unwrap_or_else(releases_page_url)
    }

    pub fn announce(&self, ux: &Ux) {
        ux.log_success(Component::Update, &self.log_message());
        ux.notice(
            vrcr_shared::console::Color::White,
            &format!(
                "Version {} is available. Type update to install it.",
                normalize_version(&self.tag)
            ),
        );
    }

    fn log_message(&self) -> String {
        let best = self;
        let channel = if best.prerelease { " (prerelease)" } else { "" };
        let link = if best.html_url.is_empty() {
            String::new()
        } else {
            format!(" ({})", best.html_url)
        };
        format!(
            "version {}{channel} is available; type update to install{link}",
            best.tag
        )
    }
}

pub async fn check<F, Fut>(include_prereleases: bool, local: &str, fetch: F) -> CheckOutcome
where
    F: FnOnce(&'static str) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let url = if include_prereleases {
        ANY_RELEASES_URL
    } else {
        STABLE_LATEST_URL
    };
    let Some(body) = fetch(url).await else {
        return CheckOutcome::Failed("GitHub could not be reached".to_owned());
    };
    let Ok(doc) = serde_json::from_str::<Value>(&body) else {
        return CheckOutcome::Failed("GitHub sent an unreadable reply".to_owned());
    };
    match available_release(&doc, include_prereleases, local) {
        Some(release) => CheckOutcome::UpdateAvailable(release),
        None => CheckOutcome::UpToDate,
    }
}

async fn github_get(url: &'static str, local: String) -> Option<String> {
    let tls = crate::mesh::connect::platform_tls().ok()?;
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(format!("VRCResolver-Watchdog/{local}"))
        .build()
        .ok()?;
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.text().await.ok()
}

pub fn start_background_check(
    include_prereleases: bool,
    prompter: Prompter,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let local = prompter.current.clone();
        check_and_offer(include_prereleases, prompter, |url| github_get(url, local)).await;
    })
}

pub fn start_manual_check(prompter: &Prompter, runtime: &tokio::runtime::Handle) -> CheckStart {
    if prompter.gate.is_running() {
        return CheckStart::Installing;
    }
    let Some(ticket) = prompter.flight.try_enter() else {
        return prompter.flight.busy_reason();
    };
    let prompter = prompter.clone();
    let include_prereleases = prompter.settings.include_prereleases();
    drop(runtime.spawn(async move {
        let local = prompter.current.clone();
        check_and_offer_manual(include_prereleases, prompter, ticket, |url| {
            github_get(url, local)
        })
        .await;
    }));
    CheckStart::Started
}

pub fn updater_command(install_dir: &Path) -> Command {
    let mut cmd = Command::new(install_dir.join(UPDATER_EXE));
    cmd.current_dir(install_dir).env(UPDATE_REQUESTED_ENV, "1");
    cmd
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateStart {
    UpdaterMissing,
    AlreadyRunning,
    Failed,
    Started,
}

pub type Spawn = fn(Command, UpdateTicket) -> io::Result<()>;

#[derive(Clone, Default)]
pub struct UpdateGate {
    running: Arc<AtomicBool>,
}

pub struct UpdateTicket {
    running: Arc<AtomicBool>,
}

impl Drop for UpdateTicket {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

impl UpdateGate {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn start(
        &self,
        install_dir: &Path,
        spawn: impl FnOnce(Command, UpdateTicket) -> io::Result<()>,
    ) -> UpdateStart {
        if !install_dir.join(UPDATER_EXE).is_file() {
            return UpdateStart::UpdaterMissing;
        }
        if self.running.swap(true, Ordering::AcqRel) {
            return UpdateStart::AlreadyRunning;
        }
        let ticket = UpdateTicket {
            running: Arc::clone(&self.running),
        };
        match spawn(updater_command(install_dir), ticket) {
            Ok(()) => UpdateStart::Started,
            Err(_) => UpdateStart::Failed,
        }
    }
}

pub fn spawn_detached(mut cmd: Command, ticket: UpdateTicket) -> io::Result<()> {
    let mut child = cmd.spawn()?;
    drop(
        thread::Builder::new()
            .name("updater-wait".to_owned())
            .spawn(move || {
                drop(child.wait());
                drop(ticket);
            }),
    );
    Ok(())
}

fn move_with_retry(src: &Path, dst: &Path) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= MOVE_RETRIES => return Err(e),
            Err(_) => {
                attempt += 1;
                thread::sleep(MOVE_RETRY_DELAY);
            }
        }
    }
}

fn apply_pair(
    install_dir: &Path,
    staged_name: &str,
    target_name: &str,
    ux: &Ux,
    log: &Log,
) -> bool {
    let staged = install_dir.join(staged_name);
    if !staged.is_file() {
        return false;
    }
    let target = install_dir.join(target_name);
    let mut backup = target.as_os_str().to_owned();
    backup.push(format!(
        ".old-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    let backup = PathBuf::from(backup);

    let result = (|| {
        if target.is_file() {
            move_with_retry(&target, &backup)?;
        }
        move_with_retry(&staged, &target)
    })();
    match result {
        Ok(()) => {
            if backup.is_file() {
                let _ = fs::remove_file(&backup);
            }
            ux.log_success(Component::Update, "updater refreshed for future updates.");
            true
        }
        Err(e) => {
            if !target.exists() && backup.is_file() {
                let _ = fs::rename(&backup, &target);
            }
            log.line(&format!(
                "[update] staged updater repair failed: {:?}: {e}",
                e.kind()
            ));
            ux.log_warn(
                Component::Update,
                "could not refresh updater yet; it will retry on next launch.",
            );
            false
        }
    }
}

pub fn apply_staged_updater(install_dir: &Path, ux: &Ux, log: &Log) -> bool {
    let mut applied = false;
    for (staged, target) in STAGED_UPDATER_PAIRS {
        if apply_pair(install_dir, staged, target, ux, log) {
            applied = true;
        }
    }
    applied
}
