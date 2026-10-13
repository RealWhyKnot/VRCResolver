use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::logutil::UtcStamp;

pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Default)]
pub struct Log {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    last_write_ms: AtomicU64,
    dev_console: AtomicBool,
}

#[derive(Default)]
struct State {
    writer: Option<File>,
    written: u64,
    dir: Option<PathBuf>,
    component: String,
}

impl Log {
    pub fn disabled() -> Log {
        Log::default()
    }

    pub fn install(dir: &Path, component: &str) -> Log {
        let log = Log::default();
        {
            let mut st = log.state();
            st.component = component.to_owned();
            if fs::create_dir_all(dir).is_ok() {
                st.dir = Some(dir.to_path_buf());
                prune(dir, component, SystemTime::now());
                open_new(&mut st);
            }
        }
        log
    }

    pub fn to_file(path: &Path) -> Log {
        let log = Log::default();
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(path) {
            let mut st = log.state();
            st.written = f.metadata().map(|m| m.len()).unwrap_or(0);
            st.writer = Some(f);
            st.component = "test".to_owned();
        }
        log
    }

    pub fn is_enabled(&self) -> bool {
        self.state().writer.is_some()
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        let st = self.state();
        let dir = st.dir.as_ref()?;
        newest(dir, &st.component)
    }

    pub fn set_dev_console(&self, on: bool) {
        self.inner.dev_console.store(on, Ordering::Relaxed);
    }

    pub fn dev_console(&self) -> bool {
        self.inner.dev_console.load(Ordering::Relaxed)
    }

    pub fn last_write(&self) -> Option<SystemTime> {
        match self.inner.last_write_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(UNIX_EPOCH + Duration::from_millis(ms)),
        }
    }

    pub fn line(&self, text: &str) {
        let now = SystemTime::now();
        let ms = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        self.inner
            .last_write_ms
            .store(u64::try_from(ms).unwrap_or(u64::MAX), Ordering::Relaxed);
        let mut st = self.state();
        let Some(w) = st.writer.as_mut() else {
            return;
        };
        let record = format!("{} {}\n", UtcStamp::at(now).log_prefix(), text);
        if w.write_all(record.as_bytes()).is_err() {
            return;
        }
        st.written += record.len() as u64;
        if st.written >= MAX_BYTES && st.dir.is_some() {
            open_new(&mut st);
        }
    }

    pub fn close(&self) {
        let mut st = self.state();
        if let Some(w) = st.writer.as_mut() {
            let _ = w.flush();
        }
        st.writer = None;
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn open_new(st: &mut State) {
    st.writer = None;
    st.written = 0;
    let Some(dir) = st.dir.clone() else {
        return;
    };
    let now = SystemTime::now();
    prune(&dir, &st.component, now);
    let name = format!("{}-{}.log", st.component, UtcStamp::at(now).file_stamp());
    st.writer = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
        .ok();
}

fn is_component_log(name: &str, component: &str) -> bool {
    name.len() > component.len() + 5
        && name.starts_with(component)
        && name.as_bytes()[component.len()] == b'-'
        && name.ends_with(".log")
}

pub fn prune(dir: &Path, component: &str, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = now.checked_sub(RETENTION).unwrap_or(UNIX_EPOCH);
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !is_component_log(&name.to_string_lossy(), component) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < cutoff);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn newest(dir: &Path, component: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| is_component_log(&e.file_name().to_string_lossy(), component))
        .map(|e| e.path())
        .max()
}
