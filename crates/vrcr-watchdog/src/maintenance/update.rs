use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::release::pick_highest_entry;
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
    pub prerelease: bool,
}

fn release_of(entry: &Value) -> Option<Release> {
    let obj = entry.as_object()?;
    let text = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Some(Release {
        tag: text("tag_name"),
        html_url: text("html_url"),
        prerelease: obj.get("prerelease") == Some(&Value::Bool(true)),
    })
}

pub fn pick_single(doc: &Value) -> Option<Release> {
    release_of(doc)
}

pub fn pick_highest(list: &Value) -> Option<Release> {
    pick_highest_entry(list).and_then(release_of)
}

pub fn available_message(doc: &Value, include_prereleases: bool, local: &str) -> Option<String> {
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
    let channel = if best.prerelease { " (prerelease)" } else { "" };
    let link = if best.html_url.is_empty() {
        String::new()
    } else {
        format!(" ({})", best.html_url)
    };
    Some(format!(
        "version {}{channel} is available; type /update to install{link}",
        best.tag
    ))
}

pub async fn check<F, Fut>(include_prereleases: bool, local: &str, fetch: F) -> Option<String>
where
    F: FnOnce(&'static str) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let url = if include_prereleases {
        ANY_RELEASES_URL
    } else {
        STABLE_LATEST_URL
    };
    let body = fetch(url).await?;
    let doc: Value = serde_json::from_str(&body).ok()?;
    available_message(&doc, include_prereleases, local)
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
    enabled: bool,
    include_prereleases: bool,
    local: &str,
    ux: Ux,
    log: &Log,
) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled {
        log.line("[update] startup update check disabled by settings");
        return None;
    }
    let local = local.to_owned();
    Some(tokio::spawn(async move {
        let fetch_version = local.clone();
        if let Some(text) = check(include_prereleases, &local, |url| github_get(url, fetch_version)).await {
            ux.success(Component::Update, &text);
        }
    }))
}

pub fn updater_command(install_dir: &Path) -> Command {
    let mut cmd = Command::new(install_dir.join(UPDATER_EXE));
    cmd.current_dir(install_dir).env(UPDATE_REQUESTED_ENV, "1");
    cmd
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateStart {
    UpdaterMissing(PathBuf),
    Failed,
    Started,
}

impl UpdateStart {
    pub fn message(&self) -> String {
        match self {
            UpdateStart::UpdaterMissing(p) => format!("updater not found in install folder: {}", p.display()),
            UpdateStart::Failed => "update could not start.".to_owned(),
            UpdateStart::Started => {
                "update started; this window may close while files are replaced.".to_owned()
            }
        }
    }
}

pub fn start_update(install_dir: &Path, spawn: impl FnOnce(Command) -> io::Result<()>) -> UpdateStart {
    let updater = install_dir.join(UPDATER_EXE);
    if !updater.is_file() {
        return UpdateStart::UpdaterMissing(updater);
    }
    match spawn(updater_command(install_dir)) {
        Ok(()) => UpdateStart::Started,
        Err(_) => UpdateStart::Failed,
    }
}

pub fn spawn_detached(mut cmd: Command) -> io::Result<()> {
    cmd.spawn().map(drop)
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

fn apply_pair(install_dir: &Path, staged_name: &str, target_name: &str, ux: &Ux, log: &Log) -> bool {
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
            ux.success(Component::Update, "updater refreshed for future updates.");
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
            ux.warn(
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
