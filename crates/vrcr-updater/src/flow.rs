use std::path::{Path, PathBuf};

use serde_json::Value;
use vrcr_shared::release::{ANY_RELEASES_URL, STABLE_LATEST_URL, Version};
use vrcr_shared::wrapper_identity::sha256_hex;

use crate::WATCHDOG_EXE;
use crate::install::{atomic_copy_over, ensure_staged_updater_copy, resolve_payload_root};
use crate::release::{LatestRelease, expected_sha, safe_file_token, select_release};

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_NO_SHA: i32 = 12;
pub const EXIT_SHA_MISMATCH: i32 = 13;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpText {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: String,
}

pub trait Host {
    fn say(&self, line: &str);
    fn err(&self, line: &str);
    fn log(&self, line: &str);
    fn file_version(&self, path: &Path) -> Option<String>;
    fn include_prereleases(&self) -> bool;
    fn get_text(&self, url: &str) -> Result<HttpText, String>;
    fn download(&self, url: &str, dest: &Path) -> Result<(), String>;
    fn extract(&self, zip: &Path, dest: &Path) -> Result<(), String>;
    fn prompt(&self) -> bool;
    fn stop_watchdog(&self);
    fn launch_watchdog(&self, exe: &Path, dir: &Path) -> Result<(), String>;
    fn temp_dir(&self) -> PathBuf;
}

pub fn current_version(host: &dyn Host, watchdog: &Path) -> Result<Version, String> {
    if !watchdog.is_file() {
        return Err(format!(
            "{WATCHDOG_EXE} was not found next to the updater (expected at {}). The install dir may be corrupt or the updater was launched from outside the install folder.",
            watchdog.display()
        ));
    }
    Ok(host
        .file_version(watchdog)
        .and_then(|v| Version::parse(v.trim()))
        .unwrap_or(Version::ZERO))
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

pub fn fetch_latest(host: &dyn Host) -> Result<(LatestRelease, Option<String>), String> {
    let include = host.include_prereleases();
    let url = if include {
        ANY_RELEASES_URL
    } else {
        STABLE_LATEST_URL
    };
    host.log(&format!(
        "[updater] GET {url}{}",
        if include { " (include-prereleases=on)" } else { "" }
    ));
    let resp = host.get_text(url)?;
    host.log(&format!(
        "[updater] response status={} content-type={}",
        resp.status,
        resp.content_type.as_deref().unwrap_or("<none>")
    ));
    if !is_success(resp.status) {
        return Err(format!("GitHub releases API returned HTTP {}.", resp.status));
    }
    let media = resp
        .content_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if !media.to_ascii_lowercase().contains("json") {
        return Err(format!(
            "GitHub releases API returned non-JSON content (Content-Type: {media}). This usually means GitHub is returning a maintenance/error page. Try again in a few minutes."
        ));
    }
    let doc: Value = serde_json::from_str(&resp.body)
        .map_err(|e| format!("GitHub releases API returned invalid JSON: {e}"))?;
    let mut log = |line: String| host.log(&line);
    let release = select_release(&doc, include, &mut log)?;
    let sha = expected_sha(
        &release,
        |url| {
            let r = host.get_text(url)?;
            if is_success(r.status) {
                Ok(r.body)
            } else {
                Err(format!("HTTP {}", r.status))
            }
        },
        &mut log,
    );
    host.log(&match &sha {
        Some(s) => format!("[updater] expected-sha256 matched ({} chars)", s.len()),
        None => "[updater] expected-sha256 absent".to_owned(),
    });
    Ok((release, sha))
}

pub fn finish_install(host: &dyn Host, payload_root: &Path, install_dir: &Path) -> Result<i32, String> {
    atomic_copy_over(payload_root, install_dir)?;
    host.say("Update installed. Relaunching watchdog...");
    host.launch_watchdog(&install_dir.join(WATCHDOG_EXE), install_dir)?;
    Ok(EXIT_OK)
}

fn update(host: &dyn Host, install_dir: &Path) -> Result<i32, String> {
    let watchdog = install_dir.join(WATCHDOG_EXE);
    host.log(&format!("[updater] start installDir={}", install_dir.display()));
    let current = current_version(host, &watchdog)?;
    host.say(&format!("Current version: {current}"));

    let (latest, sha) = fetch_latest(host)?;
    if latest.version <= current {
        host.say("You're on the latest version.");
        return Ok(EXIT_OK);
    }
    host.say(&format!("Update available: {}", latest.tag));
    let Some(expected) = sha else {
        host.err("Refusing to update: release did not provide a zip SHA256.");
        return Ok(EXIT_NO_SHA);
    };
    if !host.prompt() {
        host.say("Skipped.");
        return Ok(EXIT_OK);
    }

    let temp = host.temp_dir();
    let token = safe_file_token(&latest.tag);
    let zip = temp.join(format!("vrcresolver-{token}.zip"));
    host.download(&latest.zip_url, &zip)?;
    let actual = sha256_hex(&zip).ok_or_else(|| format!("could not hash {}", zip.display()))?;
    if !actual.eq_ignore_ascii_case(&expected) {
        host.err(&format!(
            "Refusing to install: SHA256 mismatch.\n  url={}\n  expected={expected}\n  actual={}",
            latest.zip_url,
            actual.to_ascii_uppercase()
        ));
        let _ = std::fs::remove_file(&zip);
        return Ok(EXIT_SHA_MISMATCH);
    }

    let extract = temp.join(format!("vrcresolver-extract-{}", uuid::Uuid::new_v4().simple()));
    host.extract(&zip, &extract)?;
    let payload = resolve_payload_root(&extract)?;
    ensure_staged_updater_copy(&payload)?;
    host.stop_watchdog();
    let code = finish_install(host, &payload, install_dir)?;
    let _ = std::fs::remove_file(&zip);
    let _ = std::fs::remove_dir_all(&extract);
    Ok(code)
}

pub fn run(host: &dyn Host, install_dir: &Path) -> i32 {
    match update(host, install_dir) {
        Ok(code) => code,
        Err(e) => {
            host.err(&format!("Updater failed: {e}"));
            host.log(&format!("[updater] failed: {e}"));
            EXIT_FAILED
        }
    }
}
