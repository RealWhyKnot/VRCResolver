use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use vrcr_shared::console::{
    self, Banner, Color, Component, Outcome, Overlay, ResolveStatus, Style, Ux,
};
use vrcr_shared::crash::{self, Report};
use vrcr_shared::log::{self, Log};
use vrcr_shared::logutil::UtcStamp;
use vrcr_shared::wininet::{self, DwordStore};

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?
            .extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        self.0
            .lock()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    }
}

fn fixed_clock() -> String {
    "12:34:56".to_owned()
}

fn ux(style: Style) -> (Ux, Buf) {
    let buf = Buf::default();
    (Ux::new(Box::new(buf.clone()), style, fixed_clock), buf)
}

#[derive(Default)]
struct Counts {
    clear: u32,
    render: u32,
}

struct Counting(Arc<Mutex<Counts>>);

impl Overlay for Counting {
    fn clear(&mut self, _: &mut dyn Write) -> io::Result<()> {
        if let Ok(mut c) = self.0.lock() {
            c.clear += 1;
        }
        Ok(())
    }
    fn render(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if let Ok(mut c) = self.0.lock() {
            c.render += 1;
        }
        write!(out, "> ")
    }
}

struct Broken;

impl Overlay for Broken {
    fn clear(&mut self, _: &mut dyn Write) -> io::Result<()> {
        Err(io::Error::other("gone"))
    }
    fn render(&mut self, _: &mut dyn Write) -> io::Result<()> {
        Ok(())
    }
}

fn sp(n: usize) -> String {
    " ".repeat(n)
}

fn outcome(status: ResolveStatus) -> Outcome<'static> {
    Outcome {
        height: None,
        host: "youtube.com",
        player: "avpro",
        status,
        via_cache: false,
        via_lh_yt: false,
        elapsed: Duration::from_millis(1234),
        reason: None,
        public_message: None,
    }
}

#[derive(Default)]
struct Fake {
    value: Option<u32>,
}

impl DwordStore for Fake {
    fn get(&self) -> Option<u32> {
        self.value
    }
    fn set(&mut self, v: u32) -> io::Result<()> {
        self.value = Some(v);
        Ok(())
    }
    fn delete(&mut self) -> io::Result<()> {
        self.value = None;
        Ok(())
    }
}

fn marker(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("state").join(wininet::MARKER_FILE)
}

fn run_ensure(store: &mut Fake, m: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    wininet::ensure(store, m, &mut |l| lines.push(l)).unwrap_or_else(|e| panic!("{e}"));
    lines
}

fn run_restore(store: Option<&mut Fake>, m: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let store = store.map(|s| -> &mut dyn DwordStore { s });
    wininet::restore(store, m, &mut |l| lines.push(l)).unwrap_or_else(|e| panic!("{e}"));
    lines
}

mod console_cases;

mod logging_cases;

mod wininet_cases;

mod presentation_cases;

mod notice_cases;
