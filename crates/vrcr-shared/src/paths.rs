use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::legacy;

pub const PRODUCT_DIR: &str = "vrcresolver";
pub const RENAME_MARKER: &str = ".migrated-from-wkvrcproxy";
pub const LOCALAPP_MARKER: &str = ".migrated-from-localapp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    local_app_data: PathBuf,
    program_data: PathBuf,
}

impl AppPaths {
    pub fn new(local_app_data: impl Into<PathBuf>, program_data: impl Into<PathBuf>) -> Self {
        Self {
            local_app_data: local_app_data.into(),
            program_data: program_data.into(),
        }
    }

    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_default();
        let program_data =
            std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
        Self::new(get("LOCALAPPDATA"), program_data)
    }

    pub fn local_app_data(&self) -> &Path {
        &self.local_app_data
    }

    pub fn local_low(&self) -> PathBuf {
        let local = self.local_app_data.to_string_lossy();
        let cut = local.len().saturating_sub("\\Local".len());
        match local.get(cut..) {
            Some(tail) if tail.eq_ignore_ascii_case("\\Local") => {
                PathBuf::from(format!("{}\\LocalLow", &local[..cut]))
            }
            _ => PathBuf::from(format!("{local}Low")),
        }
    }

    pub fn state_root(&self) -> PathBuf {
        self.local_low().join(PRODUCT_DIR)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.state_root().join("logs")
    }

    pub fn crashes_dir(&self) -> PathBuf {
        self.state_root().join("crashes")
    }

    pub fn program_data_root(&self) -> PathBuf {
        self.program_data.join(PRODUCT_DIR)
    }

    pub fn vrc_tools_dir(&self, custom: Option<&Path>) -> Option<PathBuf> {
        if let Some(c) = custom.filter(|c| !c.as_os_str().is_empty() && c.is_dir()) {
            return Some(c.to_path_buf());
        }
        let tools = PathBuf::from(format!("{}Low", self.local_app_data.to_string_lossy()))
            .join("VRChat")
            .join("VRChat")
            .join("Tools");
        tools.is_dir().then_some(tools)
    }

    pub fn migrate_from_legacy_product(&self, log: &mut dyn FnMut(String)) {
        let new_root = self.state_root();
        if let Err(e) = self.migrate_user_state(&new_root, log) {
            log(format!("[migrate] rename migration failed: {e}"));
        }
        let legacy_program_data = self.program_data.join(legacy::STATE_DIR_NAME);
        if let Err(e) = migrate_program_data(&legacy_program_data, &self.program_data_root(), log) {
            log(format!("[migrate] ProgramData migration failed: {e}"));
        }
    }

    fn migrate_user_state(&self, new_root: &Path, log: &mut dyn FnMut(String)) -> io::Result<()> {
        if renamed_root_already_populated(new_root) {
            return Ok(());
        }
        let legacy_low = self.local_low().join(legacy::STATE_DIR_NAME);
        let legacy_local = self.local_app_data.join(legacy::STATE_DIR_NAME);
        if !legacy_low.is_dir() && legacy_local.is_dir() {
            migrate_legacy_local_app_state(&legacy_local, &legacy_low, log);
        }
        if legacy_low.is_dir() {
            copy_legacy_root(&legacy_low, new_root, log)?;
        }
        Ok(())
    }
}

pub fn renamed_root_already_populated(new_root: &Path) -> bool {
    new_root.join(RENAME_MARKER).is_file() || dir_has_entries(new_root)
}

fn dir_has_entries(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("unix:{secs}")
}

pub fn copy_legacy_root(legacy_root: &Path, new_root: &Path, log: &mut dyn FnMut(String)) -> io::Result<()> {
    fs::create_dir_all(new_root)?;
    let mut copied = 0usize;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(entries) = fs::read_dir(legacy_root.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = rel.join(entry.file_name());
            if child
                .components()
                .next()
                .is_some_and(|c| c.as_os_str().eq_ignore_ascii_case("logs"))
            {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let dst = new_root.join(&child);
            if kind.is_dir() {
                let _ = fs::create_dir_all(&dst);
                stack.push(child);
            } else if !dst.exists() && fs::copy(entry.path(), &dst).is_ok() {
                copied += 1;
            }
        }
    }
    fs::write(new_root.join(RENAME_MARKER), now_stamp())?;
    if copied > 0 {
        log(format!(
            "[migrate] state copied from {} -> {} (files={copied})",
            legacy_root.display(),
            new_root.display()
        ));
    }
    Ok(())
}

pub fn migrate_program_data(
    legacy_root: &Path,
    new_root: &Path,
    log: &mut dyn FnMut(String),
) -> io::Result<()> {
    if !legacy_root.is_dir() || dir_has_entries(new_root) {
        return Ok(());
    }
    fs::create_dir_all(new_root)?;
    let mut copied = 0usize;
    for entry in fs::read_dir(legacy_root)?.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let dst = new_root.join(entry.file_name());
        if !dst.exists() && fs::copy(entry.path(), &dst).is_ok() {
            copied += 1;
        }
    }
    if copied > 0 {
        log(format!(
            "[migrate] machine state copied from {} -> {} (files={copied})",
            legacy_root.display(),
            new_root.display()
        ));
    }
    Ok(())
}

pub fn migrate_legacy_local_app_state(source: &Path, legacy_low: &Path, log: &mut dyn FnMut(String)) {
    if let Err(e) = try_migrate_local_app_state(source, legacy_low, log) {
        log(format!("[migrate] failed: {e}"));
    }
}

fn try_migrate_local_app_state(
    source: &Path,
    legacy_low: &Path,
    log: &mut dyn FnMut(String),
) -> io::Result<()> {
    let marker = legacy_low.join(LOCALAPP_MARKER);
    if marker.is_file() {
        return Ok(());
    }
    fs::create_dir_all(legacy_low)?;
    if !source.is_dir() {
        return fs::write(&marker, now_stamp());
    }
    let (mut moved_dirs, mut moved_files) = (0usize, 0usize);
    for entry in fs::read_dir(source)?.flatten() {
        let name = entry.file_name();
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if name.eq_ignore_ascii_case("logs") {
                let _ = fs::remove_dir_all(&path);
                continue;
            }
            let dst = legacy_low.join(&name);
            if dst.is_dir() {
                for f in fs::read_dir(&path).into_iter().flatten().flatten() {
                    let target = dst.join(f.file_name());
                    if f.file_type().is_ok_and(|t| t.is_file())
                        && !target.exists()
                        && fs::rename(f.path(), &target).is_ok()
                    {
                        moved_files += 1;
                    }
                }
                let _ = fs::remove_dir_all(&path);
            } else if fs::rename(&path, &dst).is_ok() {
                moved_dirs += 1;
            }
        }
    }
    for entry in fs::read_dir(source)?.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let dst = legacy_low.join(entry.file_name());
        if !dst.exists() && fs::rename(entry.path(), &dst).is_ok() {
            moved_files += 1;
        }
    }
    fs::write(&marker, now_stamp())?;
    let _ = fs::remove_dir_all(source);
    if moved_dirs > 0 || moved_files > 0 {
        log(format!(
            "[migrate] state moved from {} -> {} (dirs={moved_dirs}, files={moved_files})",
            source.display(),
            legacy_low.display()
        ));
    }
    Ok(())
}
