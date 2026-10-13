use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::Serialize;
use vrcr_shared::logutil::{UtcStamp, iso8601_utc};

pub const MAX_HISTORY_COMMANDS: usize = 200;
pub const MAX_RECENT_COMMANDS: usize = 30;
pub const MAX_HISTORY_BYTES: u64 = 256 * 1024;

const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

#[derive(Serialize)]
struct Event<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    time_utc: String,
    command: Option<&'a str>,
    text: Option<&'a str>,
}

pub struct SessionStore {
    dir: PathBuf,
    history_path: PathBuf,
    session_path: PathBuf,
    now: fn() -> SystemTime,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    recent: Vec<String>,
    stopped: bool,
}

impl SessionStore {
    pub fn new(dir: &Path, started: SystemTime, now: fn() -> SystemTime) -> SessionStore {
        let s = UtcStamp::at(started);
        let name = format!(
            "session-{:04}{:02}{:02}-{:02}{:02}{:02}.jsonl",
            s.year, s.month, s.day, s.hour, s.minute, s.second
        );
        SessionStore {
            dir: dir.to_path_buf(),
            history_path: dir.join("history.txt"),
            session_path: dir.join(name),
            now,
            state: Mutex::new(State::default()),
        }
    }

    pub fn under_state_root(state_root: &Path) -> SessionStore {
        SessionStore::new(&state_root.join("terminal"), SystemTime::now(), SystemTime::now)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn history_path(&self) -> &Path {
        &self.history_path
    }

    pub fn session_path(&self) -> &Path {
        &self.session_path
    }

    pub fn recent_commands(&self) -> Vec<String> {
        self.lock().recent.clone()
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

    pub fn start(&self) {
        self.record("session_start", None, Some("terminal session started"));
    }

    pub fn stop(&self) {
        {
            let mut st = self.lock();
            if st.stopped {
                return;
            }
            st.stopped = true;
        }
        self.record("session_end", None, Some("terminal session ended"));
    }

    pub fn record_command(&self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        {
            let mut st = self.lock();
            if st.recent.last().is_none_or(|last| last != command) {
                st.recent.push(command.to_owned());
                let len = st.recent.len();
                if len > MAX_RECENT_COMMANDS {
                    st.recent.drain(..len - MAX_RECENT_COMMANDS);
                }
            }
        }
        self.append_history(command);
        self.record("command", Some(command), None);
    }

    pub fn record_output(&self, level: &str, text: &str) {
        let level = level.trim();
        let kind = if level.is_empty() {
            "output".to_owned()
        } else {
            format!("output:{}", level.to_lowercase())
        };
        self.record(&kind, None, Some(text));
    }

    fn append_history(&self, command: &str) {
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

    fn record(&self, kind: &str, command: Option<&str>, text: Option<&str>) {
        let event = Event {
            kind,
            time_utc: iso8601_utc((self.now)()),
            command,
            text,
        };
        let Ok(line) = serde_json::to_string(&event) else {
            return;
        };
        let _ = fs::create_dir_all(&self.dir);
        let _ = append(&self.session_path, &format!("{line}{NEWLINE}"));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for SessionStore {
    fn drop(&mut self) {
        self.stop();
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
