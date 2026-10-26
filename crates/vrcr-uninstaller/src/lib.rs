use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use vrcr_shared::install_manifest::{MANIFEST_REL, manifest_paths, normalize_rel};
use vrcr_shared::legacy;
use vrcr_shared::logutil::UtcStamp;
use vrcr_shared::paths::AppPaths;

pub mod flow;

pub const WATCHDOG_EXE: &str = "vrcresolver.exe";
pub const SHIPPED_FILES: [&str; 7] = [
    WATCHDOG_EXE,
    "vrcresolver.Updater.exe",
    "vrcresolver.Updater.next.exe",
    "vrcresolver.Uninstaller.exe",
    "data/wrapper_hashes.txt",
    MANIFEST_REL,
    "tools/yt-dlp.exe",
];
pub const WATCHDOG_EXES: [&str; 2] = [WATCHDOG_EXE, "WKVRCProxy.exe"];
pub const WATCHDOG_PROCESSES: [&str; 2] = ["vrcresolver", "WKVRCProxy"];
pub const HOSTS_TIMEOUT: Duration = Duration::from_secs(10);
pub const TLS_TIMEOUT: Duration = Duration::from_secs(30);
pub const SETTLE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogMatch {
    Ours,
    OtherInstall,
    Uninspectable,
}

fn comparable(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.trim_end_matches('\\').to_lowercase()
}

pub fn match_watchdog(image: Option<&Path>, install_dir: &Path) -> WatchdogMatch {
    let Some(image) = image.filter(|p| !p.as_os_str().is_empty()) else {
        return WatchdogMatch::Uninspectable;
    };
    let actual = comparable(image);
    if WATCHDOG_EXES
        .iter()
        .any(|name| comparable(&install_dir.join(name)) == actual)
    {
        WatchdogMatch::Ours
    } else {
        WatchdogMatch::OtherInstall
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloseReport {
    pub matched: u32,
    pub skipped: u32,
    pub exited: u32,
    pub uninspectable: u32,
    pub stuck: u32,
}

impl CloseReport {
    pub fn summary(&self) -> String {
        format!(
            "[uninstall] close-watchdog matched={} skipped_other_installs={} already_exited={} uninspectable={}",
            self.matched, self.skipped, self.exited, self.uninspectable
        )
    }

    pub fn error(&self) -> Option<String> {
        if self.uninspectable > 0 {
            return Some(
                "a VRCResolver watchdog is running that the uninstaller can't inspect -- close it and run the uninstaller again"
                    .to_owned(),
            );
        }
        if self.stuck > 0 {
            return Some(format!(
                "{} VRCResolver watchdog process(es) did not exit -- close them and run the uninstaller again",
                self.stuck
            ));
        }
        None
    }
}

pub fn close_watchdogs(
    pids: &[u32],
    install_dir: &Path,
    image_of: &dyn Fn(u32) -> Option<PathBuf>,
    alive: &dyn Fn(u32) -> bool,
    stop: &dyn Fn(u32) -> bool,
) -> CloseReport {
    let mut report = CloseReport::default();
    for &pid in pids {
        let image = image_of(pid);
        if image.is_none() && !alive(pid) {
            report.exited += 1;
            continue;
        }
        match match_watchdog(image.as_deref(), install_dir) {
            WatchdogMatch::Uninspectable => report.uninspectable += 1,
            WatchdogMatch::OtherInstall => report.skipped += 1,
            WatchdogMatch::Ours => {
                report.matched += 1;
                if !stop(pid) {
                    report.stuck += 1;
                }
            }
        }
    }
    report
}

pub fn state_roots(paths: &AppPaths) -> Vec<PathBuf> {
    let program_data_root = paths.program_data_root();
    vec![
        paths.state_root(),
        paths.local_low().join(legacy::STATE_DIR_NAME),
        paths.local_app_data().join(legacy::STATE_DIR_NAME),
        program_data_root.clone(),
        program_data_root.with_file_name(legacy::STATE_DIR_NAME),
    ]
}

pub fn rmdir_log_name(stamp: &UtcStamp) -> String {
    format!(
        "vrcresolver-uninstall-rmdir-{:04}{:02}{:02}{:02}{:02}{:02}.log",
        stamp.year, stamp.month, stamp.day, stamp.hour, stamp.minute, stamp.second
    )
}

pub fn install_files(manifest: Option<&str>) -> Vec<PathBuf> {
    let listed = manifest.map(manifest_paths).unwrap_or_default();
    let mut files = BTreeMap::new();
    for rel in SHIPPED_FILES
        .iter()
        .filter_map(|p| normalize_rel(p))
        .chain(listed)
    {
        files.entry(comparable(&rel)).or_insert(rel);
    }
    files.into_values().collect()
}

pub fn install_subdirs(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = BTreeSet::new();
    for file in files {
        for dir in file.ancestors().skip(1).filter(|d| !d.as_os_str().is_empty()) {
            dirs.insert(dir.to_path_buf());
        }
    }
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs
}

fn cmd_safe(path: &Path, what: &str) -> Result<String, String> {
    let text = path.to_string_lossy().into_owned();
    if text.contains('"') || text.contains('%') {
        return Err(format!("{what} contains a quote or percent character: {text}"));
    }
    Ok(text)
}

pub fn self_delete_args(install_dir: &Path, exe: &Path, log: &Path) -> Result<String, String> {
    let dir = cmd_safe(install_dir, "install dir")?;
    let exe = cmd_safe(exe, "uninstaller path")?;
    let log = cmd_safe(log, "temp dir")?;
    Ok(format!(
        "/c (ping 127.0.0.1 -n 4 > nul) & (del /f /q \"{exe}\" & rmdir \"{dir}\") > \"{log}\" 2>&1"
    ))
}
