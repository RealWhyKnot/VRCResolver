use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::common::mesh::Capture;
use tempfile::TempDir;
use vrcr_shared::console::Color;
use vrcr_shared::paths::AppPaths;
use vrcr_shared::win::KeyEvent;
use vrcr_watchdog::settings::{AppSettings, Store};
use vrcr_watchdog::stats::{Activity, Bandwidth, Stats};
use vrcr_watchdog::terminal::blocks::{self, Item};
use vrcr_watchdog::terminal::commands::{Registry, edit_distance};
use vrcr_watchdog::terminal::input::{InputBuffer, MAX_INPUT_CHARS};
use vrcr_watchdog::terminal::overlay::{OverlayLine, park_caret, write_runs};
use vrcr_watchdog::terminal::renderer::{
    console_width, describe_age, describe_terminal, status_rows, usable_width,
};
use vrcr_watchdog::terminal::status::{self, FrameGate, LineInput, PROMPT};
use vrcr_watchdog::terminal::text::{
    self, ASCII, CommandLine, Frame, Run, UNICODE, activity_glyph, pulse_color, sparkline,
    trim_left, trim_right,
};
use vrcr_watchdog::terminal::{
    Core, Key, KeyDecoder, Services, SessionStore, animations_allowed, interactive_allowed,
};

fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_760_000_000)
}

fn fixed_now() -> SystemTime {
    t0()
}

fn tmp() -> TempDir {
    TempDir::new().unwrap_or_else(|e| panic!("{e}"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{e}"))
}

fn idle() -> Activity {
    Activity::default()
}

fn no_bandwidth() -> Bandwidth {
    Bandwidth {
        current: 0,
        peak: 0,
        history: vec![0; 24],
    }
}

fn line<'a>(input: &'a str, width: usize) -> LineInput<'a> {
    LineInput {
        mesh_connected: true,
        spinner: 0,
        width,
        input,
        status_line: true,
        animations: true,
        unicode: false,
        ghost: "",
        cursor: Some(input.chars().count()),
    }
}

struct Harness {
    _dir: TempDir,
    out: Capture,
    core: Core,
    settings: Arc<Store>,
    stats: Arc<Stats>,
    shutdowns: Arc<AtomicUsize>,
    session: Arc<SessionStore>,
}

fn harness_with(width: Option<usize>) -> Harness {
    harness_styled(width, vrcr_shared::console::Style::plain())
}

fn harness_styled(width: Option<usize>, style: vrcr_shared::console::Style) -> Harness {
    let dir = tmp();
    let out = Capture::default();
    let ux = out.ux_with_style(style);
    let settings = Arc::new(Store::new(
        dir.path().join("settings.json"),
        Box::new(|_| {}),
    ));
    let stats = Arc::new(Stats::new(t0()));
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let counter = std::sync::Arc::clone(&shutdowns);
    let session = Arc::new(SessionStore::new(
        &dir.path().join("terminal"),
        t0(),
        fixed_now,
    ));
    let width_fn: fn() -> Option<usize> = if width.is_some() {
        || Some(100)
    } else {
        || None
    };
    let services = Services {
        ux,
        stats: std::sync::Arc::clone(&stats),
        settings: std::sync::Arc::clone(&settings),
        mesh_connected: Arc::new(|| true),
        request_shutdown: Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
        install_dir: dir.path().join("install"),
        paths: AppPaths::new(dir.path().join("local"), dir.path().join("data")),
        spawn_updater: |_| Ok(()),
        width: width_fn,
        animations_available: false,
        now: fixed_now,
    };
    let core = Core::new(services, std::sync::Arc::clone(&session));
    Harness {
        _dir: dir,
        out,
        core,
        settings,
        stats,
        shutdowns,
        session,
    }
}

fn harness() -> Harness {
    harness_with(None)
}

impl Harness {
    fn typed(&self, text: &str) {
        for c in text.chars() {
            self.core.handle_key(Key::Char(c));
        }
    }

    fn run(&self, text: &str) {
        self.typed(text);
        self.core.handle_key(Key::Enter);
    }
}

mod commands_cases;

mod history_cases;

mod input_cases;

mod rendering_cases;

mod settings_cases;

mod text_cases;
