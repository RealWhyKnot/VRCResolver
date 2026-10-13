use std::fs;
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::logutil::{UtcStamp, iso8601_utc};

pub type Snapshot = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Clone)]
pub struct Crash {
    snapshot: Arc<Mutex<Option<Snapshot>>>,
}

pub struct Report<'a> {
    pub component: &'a str,
    pub kind: &'a str,
    pub terminating: bool,
    pub pid: u32,
    pub version: &'a str,
    pub basedir: &'a str,
    pub os: &'a str,
    pub snapshot: Option<String>,
    pub detail: &'a str,
}

impl Crash {
    pub fn install(dir: PathBuf, component: &str, version: &str) -> Crash {
        let crash = Crash {
            snapshot: Arc::new(Mutex::new(None)),
        };
        if fs::create_dir_all(&dir).is_err() {
            return crash;
        }
        let snapshot = crash.snapshot.clone();
        let component = component.to_owned();
        let version = version.to_owned();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let snap = snapshot.lock().ok().and_then(|g| g.clone());
            write(&dir, &component, &version, info, snap.as_ref());
            previous(info);
        }));
        crash
    }

    pub fn set_snapshot(&self, snapshot: Option<Snapshot>) {
        if let Ok(mut g) = self.snapshot.lock() {
            *g = snapshot;
        }
    }
}

fn write(dir: &Path, component: &str, version: &str, info: &PanicHookInfo<'_>, snap: Option<&Snapshot>) {
    let now = SystemTime::now();
    let path = dir.join(format!(
        "crash-{component}-{}.log",
        UtcStamp::at(now).file_stamp()
    ));
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-string panic payload)".to_owned());
    let location = info
        .location()
        .map(|l| format!("{}:{}", l.file(), l.line()))
        .unwrap_or_default();
    let thread = std::thread::current();
    let detail = format!(
        "panic in thread '{}' at {location}: {message}\n\n{}",
        thread.name().unwrap_or("?"),
        std::backtrace::Backtrace::force_capture()
    );
    let basedir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.display().to_string()))
        .unwrap_or_default();
    let report = Report {
        component,
        kind: "panic",
        terminating: true,
        pid: std::process::id(),
        version,
        basedir: &basedir,
        os: std::env::consts::OS,
        snapshot: snap.map(|f| f()),
        detail: &detail,
    };
    let profile = std::env::var("USERPROFILE").ok();
    let user = std::env::var("USERNAME").ok();
    let text = redact(&render(&report, now), profile.as_deref(), user.as_deref());
    if let Ok(mut f) = fs::File::create(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

pub fn render(r: &Report<'_>, at: SystemTime) -> String {
    let mut s = String::new();
    s.push_str("=== vrcresolver crash log ===\n");
    s.push_str(&format!("timestamp:    {}\n", iso8601_utc(at)));
    s.push_str(&format!("component:    {}\n", r.component));
    s.push_str(&format!("kind:         {}\n", r.kind));
    s.push_str(&format!(
        "terminating:  {}\n",
        if r.terminating { "True" } else { "False" }
    ));
    s.push_str(&format!("pid:          {}\n", r.pid));
    s.push_str(&format!("version:      {}\n", r.version));
    s.push_str(&format!("basedir:      {}\n", r.basedir));
    s.push_str(&format!("os:           {}\n", r.os));
    if let Some(snap) = &r.snapshot {
        s.push_str("\n--- state snapshot ---\n");
        s.push_str(snap);
        s.push('\n');
    }
    s.push('\n');
    s.push_str(r.detail);
    s.push('\n');
    s
}

pub fn redact(text: &str, user_profile: Option<&str>, user_name: Option<&str>) -> String {
    let mut out = text.to_owned();
    if let Some(p) = user_profile.filter(|p| !p.is_empty()) {
        out = replace_ignore_case(&out, p, "%USERPROFILE%");
    }
    if let Some(u) = user_name.filter(|u| u.chars().count() >= 3) {
        out = replace_ignore_case(&out, u, "<user>");
    }
    out
}

fn replace_ignore_case(haystack: &str, needle: &str, with: &str) -> String {
    let lower_hay = haystack.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    if lower_needle.is_empty() {
        return haystack.to_owned();
    }
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while let Some(off) = lower_hay[i..].find(&lower_needle) {
        out.push_str(&haystack[i..i + off]);
        out.push_str(with);
        i += off + lower_needle.len();
    }
    out.push_str(&haystack[i..]);
    out
}
