use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::Duration;

use vrcr_shared::install_manifest::manifest_paths;
pub use vrcr_shared::install_manifest::normalize_rel;

use crate::{LEGACY_WATCHDOG_EXE, MANIFEST_REL, STAGED_UPDATER_EXE, UPDATER_EXE, WATCHDOG_EXE};

const MOVE_RETRIES: u32 = 3;
const MOVE_RETRY_DELAY: Duration = Duration::from_millis(200);

pub fn safe_install_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = normalize_rel(relative).ok_or_else(|| format!("Unsafe release manifest path: {relative}"))?;
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!("Release path escapes install directory: {relative}"));
    }
    Ok(root.join(rel))
}

fn key(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/").to_lowercase()
}

pub fn read_manifest_paths(root: &Path) -> BTreeMap<String, String> {
    let mut paths = BTreeMap::new();
    let Ok(manifest) = safe_install_path(root, MANIFEST_REL) else {
        return paths;
    };
    let Ok(text) = fs::read_to_string(manifest) else {
        return paths;
    };
    for rel in manifest_paths(&text) {
        paths.insert(key(&rel), rel.to_string_lossy().replace('\\', "/"));
    }
    paths
}

fn contains_watchdog(dir: &Path) -> bool {
    dir.join(WATCHDOG_EXE).is_file() || dir.join(LEGACY_WATCHDOG_EXE).is_file()
}

pub fn resolve_payload_root(extract_root: &Path) -> Result<PathBuf, String> {
    if contains_watchdog(extract_root) {
        return Ok(extract_root.to_path_buf());
    }
    let candidates: Vec<PathBuf> = fs::read_dir(extract_root)
        .map_err(|e| format!("could not list {}: {e}", extract_root.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && contains_watchdog(p))
        .collect();
    match candidates.as_slice() {
        [only] => Ok(only.clone()),
        _ => Err(format!(
            "Release zip did not contain a recognizable payload root for {WATCHDOG_EXE}."
        )),
    }
}

pub fn ensure_staged_updater_copy(payload_root: &Path) -> Result<(), String> {
    let updater = payload_root.join(UPDATER_EXE);
    if !updater.is_file() {
        return Ok(());
    }
    fs::copy(&updater, payload_root.join(STAGED_UPDATER_EXE))
        .map(drop)
        .map_err(|e| format!("Failed to stage updater refresh copy in extracted payload: {e}"))
}

fn files_under(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn suffixed(path: &Path, tag: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(format!(
        ".{tag}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    PathBuf::from(name)
}

pub fn move_with_retry(src: &Path, dst: &Path) -> io::Result<()> {
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

fn name_is(path: &Path, name: &str) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
}

fn replace_error(dst: &Path, e: &io::Error) -> String {
    if name_is(dst, "yt-dlp.exe") {
        return format!(
            "Failed to replace {} -- file is locked. Close VRChat (it may be holding yt-dlp.exe) and re-run the updater. ({e})",
            dst.display()
        );
    }
    if name_is(dst, WATCHDOG_EXE) || name_is(dst, LEGACY_WATCHDOG_EXE) {
        return format!(
            "Failed to replace {} -- watchdog process may not have fully exited yet. Wait a few seconds and re-run the updater. ({e})",
            dst.display()
        );
    }
    format!("Failed to replace {}: {e}", dst.display())
}

fn stage(from: &Path, to: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = (|| {
        for file in files_under(from).map_err(|e| format!("could not list {}: {e}", from.display()))? {
            let rel = file
                .strip_prefix(from)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned();
            let target = safe_install_path(to, &rel)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
            }
            if name_is(&target, UPDATER_EXE) {
                continue;
            }
            let temp = suffixed(&target, "new");
            fs::copy(&file, &temp).map_err(|e| format!("could not stage {}: {e}", target.display()))?;
            staged.push((temp, target));
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(staged),
        Err(e) => {
            for (temp, _) in &staged {
                let _ = fs::remove_file(temp);
            }
            Err(e)
        }
    }
}

fn swap(
    staged: &[(PathBuf, PathBuf)],
    deletes: &[PathBuf],
    renamed: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    for (temp, dst) in staged {
        if dst.is_file() {
            let backup = suffixed(dst, "old");
            move_with_retry(dst, &backup).map_err(|e| replace_error(dst, &e))?;
            renamed.push((backup, dst.clone()));
        }
        move_with_retry(temp, dst).map_err(|e| replace_error(dst, &e))?;
    }
    for dst in deletes {
        if !dst.is_file() {
            continue;
        }
        let backup = suffixed(dst, "old");
        move_with_retry(dst, &backup).map_err(|e| replace_error(dst, &e))?;
        renamed.push((backup, dst.clone()));
    }
    Ok(())
}

fn rollback(renamed: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (backup, dst) in renamed {
        let restored = (|| {
            if dst.is_file() {
                fs::remove_file(dst)?;
            }
            fs::rename(backup, dst)
        })();
        if let Err(e) = restored {
            failures.push(format!("{} (backup at {}): {e}", dst.display(), backup.display()));
        }
    }
    failures
}

pub fn atomic_copy_over(from: &Path, to: &Path) -> Result<(), String> {
    let new_paths = read_manifest_paths(from);
    let old_paths = read_manifest_paths(to);
    let deletes: Vec<PathBuf> = if old_paths.is_empty() || new_paths.is_empty() {
        Vec::new()
    } else {
        old_paths
            .iter()
            .filter(|(k, _)| !new_paths.contains_key(*k))
            .map(|(_, rel)| safe_install_path(to, rel))
            .collect::<Result<_, _>>()?
    };

    let staged = stage(from, to)?;
    let mut renamed = Vec::new();
    if let Err(primary) = swap(&staged, &deletes, &mut renamed) {
        let failures = rollback(&renamed);
        for (temp, _) in &staged {
            let _ = fs::remove_file(temp);
        }
        if failures.is_empty() {
            return Err(primary);
        }
        let list: Vec<String> = failures.iter().map(|f| format!("  - {f}")).collect();
        return Err(format!(
            "{primary}\nRollback ALSO failed for {} file(s) -- install may be inconsistent. Manual recovery: in the install dir, find each *.old-<8hex> file listed below and rename it back to its original name (drop the .old-* suffix).\n{}",
            failures.len(),
            list.join("\n")
        ));
    }
    for (backup, _) in &renamed {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}
