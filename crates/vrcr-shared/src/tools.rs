use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::log::Log;
use crate::logutil::UtcStamp;
use crate::wrapper_identity::{self, WrapperKind};

pub const TARGET_NAME: &str = "yt-dlp.exe";
pub const OG_NAME: &str = "yt-dlp-og.exe";
const WRAPPER_LOG: &str = "yt-dlp-wrapper.log";
const SIDECAR_PREFIXES: [&str; 4] = [
    "yt-dlp.exe.new-",
    "yt-dlp.exe.stale-",
    "yt-dlp-og.exe.new-",
    "yt-dlp-og.exe.stale-",
];
const LEGACY_INSTALL_TOOLS: [&str; 2] = ["yt-dlp-og-fallback.exe", "yt-dlp-og-fallback.version.txt"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    Restored,
    KeptNewerVrchatBinary,
    RemovedOrphanWrapper,
    NothingToDo,
    Failed,
}

pub trait RestoreOps {
    fn move_file(&self, from: &Path, to: &Path, overwrite: bool) -> io::Result<()>;
    fn hash(&self, path: &Path) -> Option<String>;
}

pub struct RealOps;

impl RestoreOps for RealOps {
    fn move_file(&self, from: &Path, to: &Path, overwrite: bool) -> io::Result<()> {
        move_file(from, to, overwrite)
    }

    fn hash(&self, path: &Path) -> Option<String> {
        wrapper_identity::sha256_hex(path)
    }
}

pub fn move_file(from: &Path, to: &Path, overwrite: bool) -> io::Result<()> {
    if !overwrite && fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    fs::rename(from, to)
}

pub fn restore(
    tools_dir: &Path,
    known_hashes: Option<&Path>,
    ops: &dyn RestoreOps,
) -> (RestoreOutcome, String) {
    if tools_dir.as_os_str().is_empty() || !tools_dir.is_dir() {
        return (RestoreOutcome::Failed, "tools folder not found".to_owned());
    }
    let target = tools_dir.join(TARGET_NAME);
    let og = tools_dir.join(OG_NAME);
    let target_exists = target.is_file();

    if !og.is_file() {
        if !target_exists {
            return (
                RestoreOutcome::NothingToDo,
                "no yt-dlp.exe or yt-dlp-og.exe in Tools".to_owned(),
            );
        }
        if wrapper_identity::classify(&target, known_hashes) != WrapperKind::Ours {
            return (
                RestoreOutcome::NothingToDo,
                "yt-dlp-og.exe is gone and yt-dlp.exe is not our wrapper".to_owned(),
            );
        }
        return if discard(&target) {
            (
                RestoreOutcome::RemovedOrphanWrapper,
                "yt-dlp-og.exe is gone; removed our wrapper".to_owned(),
            )
        } else {
            (
                RestoreOutcome::Failed,
                "yt-dlp-og.exe is gone and our wrapper could not be removed".to_owned(),
            )
        };
    }

    if target_exists
        && wrapper_identity::classify(&target, known_hashes) == WrapperKind::VrcBundledYtDlp
        && wrapper_identity::looks_complete(&target)
        && known_to_differ(ops, &target, &og)
    {
        let detail = if discard(&og) {
            "yt-dlp.exe is a newer VRChat binary; dropped yt-dlp-og.exe"
        } else {
            "yt-dlp.exe is a newer VRChat binary; yt-dlp-og.exe could not be dropped"
        };
        return (RestoreOutcome::KeptNewerVrchatBinary, detail.to_owned());
    }

    if !target_exists {
        return match ops.move_file(&og, &target, false) {
            Ok(()) => (
                RestoreOutcome::Restored,
                "moved yt-dlp-og.exe into place".to_owned(),
            ),
            Err(e) => (RestoreOutcome::Failed, format!("restore failed: {}", kind(&e))),
        };
    }

    if ops.move_file(&og, &target, true).is_ok() {
        return (
            RestoreOutcome::Restored,
            "moved yt-dlp-og.exe over yt-dlp.exe".to_owned(),
        );
    }

    if !og.is_file() {
        return (
            RestoreOutcome::Failed,
            "yt-dlp-og.exe disappeared; yt-dlp.exe left alone".to_owned(),
        );
    }

    let stale = stale_path(&target);
    if let Err(e) = move_file(&target, &stale, false) {
        return (
            RestoreOutcome::Failed,
            format!("yt-dlp.exe is locked and could not be moved aside: {}", kind(&e)),
        );
    }

    match ops.move_file(&og, &target, false) {
        Ok(()) => (
            RestoreOutcome::Restored,
            format!(
                "yt-dlp.exe was in use; moved aside as {} and moved yt-dlp-og.exe into place",
                file_name(&stale)
            ),
        ),
        Err(cause) => (RestoreOutcome::Failed, put_back(&stale, &target, &cause)),
    }
}

fn put_back(stale: &Path, target: &Path, cause: &io::Error) -> String {
    let prefix = format!("could not move yt-dlp-og.exe into place ({}); ", kind(cause));
    match move_file(stale, target, false) {
        Ok(()) => prefix + "yt-dlp.exe put back",
        Err(e) => format!("{prefix}yt-dlp.exe left as {} ({})", file_name(stale), kind(&e)),
    }
}

fn discard(path: &Path) -> bool {
    fs::remove_file(path).is_ok() || move_file(path, &stale_path(path), false).is_ok()
}

fn known_to_differ(ops: &dyn RestoreOps, a: &Path, b: &Path) -> bool {
    let Some(ha) = ops.hash(a) else {
        return false;
    };
    ops.hash(b).is_some_and(|hb| hb != ha)
}

fn stale_path(path: &Path) -> PathBuf {
    let s = UtcStamp::at(SystemTime::now());
    let stamp = format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}",
        s.year, s.month, s.day, s.hour, s.minute, s.second
    );
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".stale-{stamp}"));
    PathBuf::from(name)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn kind(e: &io::Error) -> String {
    format!("{:?}", e.kind())
}

fn lower_name(path: &Path) -> String {
    file_name(path).to_ascii_lowercase()
}

pub fn is_sidecar(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SIDECAR_PREFIXES.iter().any(|p| lower.starts_with(p)) || lower == WRAPPER_LOG
}

pub fn pyinstaller_temp_pid(name: &str) -> Option<u32> {
    let head = name.get(..4)?;
    let digits = name.get(4..)?;
    if !head.eq_ignore_ascii_case("_mei") || digits.is_empty() || digits.len() > 9 {
        return None;
    }
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub fn sweep(tools_dir: &Path, log: &Log, alive: &dyn Fn(u32) -> bool) {
    if tools_dir.as_os_str().is_empty() || !tools_dir.is_dir() {
        return;
    }
    restore_bundled_stale(tools_dir, log);

    if let Ok(entries) = fs::read_dir(tools_dir) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file() && is_sidecar(&file_name(&path)) {
                let _ = fs::remove_file(&path);
            }
        }
    }

    if let Ok(entries) = fs::read_dir(tools_dir) {
        for e in entries.flatten() {
            let path = e.path();
            if !path.is_dir() {
                continue;
            }
            let Some(pid) = pyinstaller_temp_pid(&file_name(&path)) else {
                continue;
            };
            if pid == 0 || alive(pid) {
                continue;
            }
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn restore_bundled_stale(tools_dir: &Path, log: &Log) {
    let target = tools_dir.join(TARGET_NAME);
    if target.exists() || tools_dir.join(OG_NAME).exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(tools_dir) else {
        return;
    };
    let newest = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && lower_name(p).starts_with("yt-dlp.exe.stale-"))
        .max_by(|a, b| file_name(a).cmp(&file_name(b)));
    let Some(newest) = newest else {
        return;
    };
    if wrapper_identity::classify(&newest, None) != WrapperKind::VrcBundledYtDlp {
        return;
    }
    if move_file(&newest, &target, false).is_ok() {
        log.line(&format!(
            "[patch] sweep: put VRChat's yt-dlp.exe back from {}",
            file_name(&newest)
        ));
    }
}

pub fn sweep_legacy_install_tools(install_dir: &Path) {
    if install_dir.as_os_str().is_empty() {
        return;
    }
    let tools = install_dir.join("tools");
    if !tools.is_dir() {
        return;
    }
    for name in LEGACY_INSTALL_TOOLS {
        let path = tools.join(name);
        if path.is_file() {
            let _ = fs::remove_file(path);
        }
    }
}
