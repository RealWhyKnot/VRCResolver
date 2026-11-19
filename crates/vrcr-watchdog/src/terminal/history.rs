use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MAX_HISTORY_COMMANDS: usize = 200;
pub const MAX_HISTORY_BYTES: u64 = 256 * 1024;

const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

pub struct HistoryStore {
    dir: PathBuf,
    history_path: PathBuf,
}

impl HistoryStore {
    pub fn new(dir: &Path) -> HistoryStore {
        HistoryStore {
            dir: dir.to_path_buf(),
            history_path: dir.join("history.txt"),
        }
    }

    pub fn history_path(&self) -> &Path {
        &self.history_path
    }

    pub fn load_history(&self) -> Vec<String> {
        let Ok(meta) = fs::metadata(&self.history_path) else {
            return Vec::new();
        };
        if meta.len() > MAX_HISTORY_BYTES {
            return Vec::new();
        }
        let Ok(text) = fs::read_to_string(&self.history_path) else {
            return Vec::new();
        };
        last_non_blank(&text, MAX_HISTORY_COMMANDS)
            .into_iter()
            .map(|s| s.trim().to_owned())
            .collect()
    }

    pub fn record_command(&self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        let _ = fs::create_dir_all(&self.dir);
        if append(&self.history_path, &format!("{command}{NEWLINE}")).is_ok() {
            self.trim_history();
        }
    }

    fn trim_history(&self) {
        let Ok(meta) = fs::metadata(&self.history_path) else {
            return;
        };
        if meta.len() <= MAX_HISTORY_BYTES {
            return;
        }
        let Ok(text) = fs::read_to_string(&self.history_path) else {
            return;
        };
        let mut out = String::new();
        for line in last_non_blank(&text, MAX_HISTORY_COMMANDS) {
            out.push_str(line);
            out.push_str(NEWLINE);
        }
        let _ = fs::write(&self.history_path, out);
    }
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

fn last_non_blank(text: &str, max: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(max);
    lines[start..].to_vec()
}
