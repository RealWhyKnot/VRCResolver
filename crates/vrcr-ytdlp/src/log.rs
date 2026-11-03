use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use vrcr_shared::logutil::{bare_host, iso8601_utc, sanitize_for_console};

pub const LOG_FILE: &str = "yt-dlp-wrapper.log";
pub const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

pub struct ShimLog {
    path: Option<PathBuf>,
    rid: String,
    writer: Option<File>,
    failed: bool,
}

impl ShimLog {
    pub fn new(path: Option<PathBuf>, rid: String) -> Self {
        Self {
            path,
            rid,
            writer: None,
            failed: false,
        }
    }

    pub fn rid(&self) -> &str {
        &self.rid
    }

    pub fn line(&mut self, message: &str) {
        let message = sanitize_for_console(message, usize::MAX);
        let line = format!(
            "[{}] [{}] {message}\n",
            iso8601_utc(SystemTime::now()),
            self.rid
        );
        if let Some(w) = self.writer() {
            let _ = w.write_all(line.as_bytes());
            let _ = w.flush();
        }
    }

    fn writer(&mut self) -> Option<&mut File> {
        if self.writer.is_none() && !self.failed {
            self.writer = self.open();
            self.failed = self.writer.is_none();
        }
        self.writer.as_mut()
    }

    fn open(&self) -> Option<File> {
        let path = self.path.as_ref()?;
        fs::create_dir_all(path.parent()?).ok()?;
        if fs::metadata(path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
            let mut old = path.clone().into_os_string();
            old.push(".old");
            let _ = fs::rename(path, old);
        }
        OpenOptions::new().create(true).append(true).open(path).ok()
    }
}

pub fn start_banner(pid: u32, version: &str, args: &[String], url: &str, player: &str) -> String {
    let host = if url.is_empty() {
        "<none>".to_owned()
    } else {
        bare_host(url)
    };
    format!(
        "START pid={pid} ver={version} argc={} url-host={host} player={player}",
        args.len()
    )
}

pub fn is_http(arg: &str) -> bool {
    let starts = |p: &str| {
        arg.get(..p.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(p))
    };
    starts("http://") || starts("https://")
}

pub fn extract_url(args: &[String]) -> &str {
    args.iter().find(|a| is_http(a)).map_or("", String::as_str)
}
