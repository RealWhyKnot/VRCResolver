use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const STALE_AFTER: Duration = Duration::from_secs(24 * 3600);
const DIR_PREFIXES: [&str; 2] = ["vrcresolver-extract-", "wkvrcproxy-extract-"];
const ZIP_PREFIXES: [&str; 2] = ["vrcresolver-", "wkvrcproxy-"];

pub fn is_stale_extract_dir(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    DIR_PREFIXES.iter().any(|p| lower.starts_with(p))
}

pub fn is_stale_zip(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".zip")
        && ZIP_PREFIXES
            .iter()
            .any(|p| lower.starts_with(p) && lower.len() >= p.len() + 4)
}

pub fn sweep(temp_dir: &Path, now: SystemTime) -> usize {
    let Some(cutoff) = now.checked_sub(STALE_AFTER) else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(temp_dir) else {
        return 0;
    };
    let mut swept = 0;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.modified().is_ok_and(|m| m < cutoff) {
            continue;
        }
        let path = entry.path();
        let removed = if meta.is_dir() && is_stale_extract_dir(&name) {
            fs::remove_dir_all(&path).is_ok()
        } else if meta.is_file() && is_stale_zip(&name) {
            fs::remove_file(&path).is_ok()
        } else {
            false
        };
        if removed {
            swept += 1;
        }
    }
    swept
}
