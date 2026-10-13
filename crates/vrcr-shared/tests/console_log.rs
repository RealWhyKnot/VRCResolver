use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use vrcr_shared::console::{self, Banner, Color, Component, Outcome, Overlay, ResolveStatus, Style, Ux};
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
            .map_err(|_| io::Error::other("poisoned"))?
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

#[test]
fn write_clears_and_restores_the_active_overlay() {
    let (ux, buf) = ux(Style::plain());
    let counts = Arc::new(Mutex::new(Counts::default()));
    let guard = ux.use_overlay(Box::new(Counting(counts.clone())));
    assert_eq!(counts.lock().map(|c| c.render).unwrap_or(0), 1);

    ux.write(Component::Terminal, "hello");

    let (clear, render) = counts.lock().map(|c| (c.clear, c.render)).unwrap_or_default();
    assert_eq!((clear, render), (1, 2));
    assert!(buf.text().contains("[12:34:56] * [terminal] hello\n"));

    drop(guard);
    let cleared = counts.lock().map(|c| c.clear).unwrap_or(0);
    assert_eq!(cleared, 2);
    ux.write(Component::Mesh, "after");
    assert_eq!(counts.lock().map(|c| c.render).unwrap_or(0), 2);
}

#[test]
fn a_stale_overlay_guard_does_not_remove_a_newer_overlay() {
    let (ux, _) = ux(Style::plain());
    let first = Arc::new(Mutex::new(Counts::default()));
    let second = Arc::new(Mutex::new(Counts::default()));
    let old = ux.use_overlay(Box::new(Counting(first)));
    let _new = ux.use_overlay(Box::new(Counting(second.clone())));
    drop(old);
    ux.write(Component::Ipc, "x");
    assert_eq!(second.lock().map(|c| c.render).unwrap_or(0), 2);
}

#[test]
fn a_failing_overlay_is_dropped_and_writes_continue() {
    let (ux, buf) = ux(Style::plain());
    let _g = ux.use_overlay(Box::new(Broken));
    ux.write(Component::Relay, "one");
    ux.write(Component::Relay, "two");
    assert!(buf.text().contains("[relay] one"));
    assert!(buf.text().contains("[relay] two"));
}

#[test]
fn levels_use_their_tags_and_colors() {
    let (ux, buf) = ux(Style {
        color: true,
        unicode: true,
    });
    ux.warn(Component::Hosts, "w");
    ux.error(Component::Patch, "e");
    ux.success(Component::Update, "s");
    ux.fatal("f");
    let text = buf.text();
    assert!(text.contains(&format!(
        "{}[12:34:56] \u{2022} [hosts][warn] w\x1b[0m",
        Color::Yellow.sgr()
    )));
    assert!(text.contains(&format!(
        "{}[12:34:56] \u{2022} [patch][err] e\x1b[0m",
        Color::Red.sgr()
    )));
    assert!(text.contains(&format!(
        "{}[12:34:56] \u{2022} [update] s\x1b[0m",
        Color::Green.sgr()
    )));
    assert!(text.contains("\u{2022} [FATAL] f"));
}

#[test]
fn style_detection_follows_redirect_no_color_and_ascii() {
    assert_eq!(Style::detect(true, None, None), Style::plain());
    assert_eq!(
        Style::detect(false, None, None),
        Style {
            color: true,
            unicode: true
        }
    );
    assert_eq!(
        Style::detect(false, Some(""), None),
        Style {
            color: true,
            unicode: true
        }
    );
    assert_eq!(
        Style::detect(false, Some("1"), None),
        Style {
            color: false,
            unicode: true
        }
    );
    assert_eq!(
        Style::detect(false, None, Some("TRUE")),
        Style {
            color: true,
            unicode: false
        }
    );
    assert_eq!(
        Style::detect(false, None, Some("1")),
        Style {
            color: true,
            unicode: false
        }
    );
    assert_eq!(
        Style::detect(false, None, Some("0")),
        Style {
            color: true,
            unicode: true
        }
    );
}

#[test]
fn every_component_has_a_bracketed_tag() {
    use Component::*;
    for c in [
        Mesh, Ipc, Hosts, Patch, Wrapper, Heartbeat, Relay, Terminal, Codec, Report, Update, VrcLog, YtDlp,
        Shutdown,
    ] {
        let t = c.tag();
        assert!(t.starts_with('[') && t.ends_with(']') && t.len() > 2, "{c:?}");
    }
    assert_eq!(YtDlp.tag(), "[yt-dlp]");
    assert_eq!(Shutdown.color(), Color::DarkRed);
}

fn sp(n: usize) -> String {
    " ".repeat(n)
}

fn outcome(status: ResolveStatus) -> Outcome<'static> {
    Outcome {
        host: "youtube.com",
        player: "avpro",
        status,
        via_cache: false,
        via_lh_yt: false,
        elapsed: Duration::from_millis(1234),
        reason: None,
    }
}

#[test]
fn resolve_outcome_lines_keep_their_columns() {
    let (c, line) = console::resolve_outcome_line("*", &outcome(ResolveStatus::Resolved));
    assert_eq!(c, Color::Green);
    assert_eq!(
        line,
        format!(
            "* [resolve] OK  resolved  youtube.com{}avpro{}1.2s",
            sp(24),
            sp(13)
        )
    );

    let cached = Outcome {
        via_cache: true,
        via_lh_yt: true,
        ..outcome(ResolveStatus::Resolved)
    };
    let (_, line) = console::resolve_outcome_line("*", &cached);
    assert_eq!(
        line,
        format!(
            "* [resolve] OK  cached    youtube.com{} [yt]  avpro{}1.2s",
            sp(17),
            sp(13)
        )
    );

    let failed = Outcome {
        reason: Some("timeout"),
        elapsed: Duration::from_secs(125),
        host: "a-very-long-host-name-that-overflows.example.com",
        ..outcome(ResolveStatus::Failed)
    };
    let (c, line) = console::resolve_outcome_line("*", &failed);
    assert_eq!(c, Color::Red);
    assert_eq!(
        line,
        format!(
            "* [resolve] XX  failed    a-very-long-host-name-that..{}avpro{}2m5s   timeout",
            sp(7),
            sp(13)
        )
    );

    let (c, line) = console::resolve_outcome_line("*", &outcome(ResolveStatus::Fallback));
    assert_eq!(c, Color::Yellow);
    assert!(line.starts_with("* [resolve] !!  fallback  "));
    let (c, line) = console::resolve_outcome_line(
        "*",
        &Outcome {
            reason: Some(""),
            ..outcome(ResolveStatus::Unexpected)
        },
    );
    assert_eq!(c, Color::DarkGray);
    assert!(line.starts_with("* [resolve] ??  unknown   ") && line.ends_with("1.2s"));
}

#[test]
fn truncate_pad_and_elapsed_match_the_old_console() {
    assert_eq!(console::truncate_pad("abc", 5), "abc  ");
    assert_eq!(console::truncate_pad("abcdef", 5), "abc..");
    assert_eq!(console::truncate_pad("abcdef", 3), "abc");
    assert_eq!(
        console::truncate_pad("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 5),
        "\u{e9}\u{e9}\u{e9}.."
    );
    assert_eq!(console::format_elapsed(Duration::ZERO), "0.0s");
    assert_eq!(console::format_elapsed(Duration::from_millis(59_940)), "59.9s");
    assert_eq!(console::format_elapsed(Duration::from_secs(60)), "1m0s");
    assert_eq!(console::format_elapsed(Duration::from_millis(3_725_900)), "62m5s");
}

#[test]
fn phase_divider_centres_the_label_in_sixty_columns() {
    let line = console::phase_divider_line("startup");
    assert_eq!(line.len(), 60);
    assert_eq!(line, format!("{} startup {}", "-".repeat(25), "-".repeat(26)));
    let long = "x".repeat(70);
    assert_eq!(console::phase_divider_line(&long), format!(" {long} "));
}

#[test]
fn banner_pads_labels_and_flags_dev_mode() {
    let paths = [
        ("logs", "C:\\logs".to_owned()),
        ("state root", "C:\\state".to_owned()),
    ];
    let b = Banner {
        version: "2026.10.14.0",
        sha: "abc1234",
        build_time: "now",
        dev: true,
        paths: &paths,
    };
    let lines: Vec<String> = console::banner_lines(&b).into_iter().map(|(_, l)| l).collect();
    assert_eq!(lines[1], "  VRCResolver 2026.10.14.0");
    assert!(lines[4].contains("mode: DEV"));
    assert_eq!(lines[6], "  logs:        C:\\logs");
    assert_eq!(lines[7], "  state root:  C:\\state");
    assert_eq!(lines.last().map(String::as_str), Some(""));
    let quiet = Banner { dev: false, ..b };
    assert!(
        !console::banner_lines(&quiet)
            .iter()
            .any(|(_, l)| l.contains("DEV"))
    );
}

#[test]
fn wrapper_fallback_line_is_stamped_and_yellow() {
    let (ux, buf) = ux(Style {
        color: true,
        unicode: false,
    });
    ux.wrapper_fallback("youtube.com", "pipe_down", 42);
    assert_eq!(
        buf.text(),
        format!(
            "{}[12:34:56] * [wrapper] !!  fallback  youtube.com{}pipe_down   elapsed=42ms\x1b[0m\n",
            Color::Yellow.sgr(),
            sp(24)
        )
    );
}

#[test]
fn utc_stamps_format_like_the_old_logger() {
    let at = UNIX_EPOCH + Duration::from_millis(1_760_400_000_123);
    let s = UtcStamp::at(at);
    assert_eq!(s.log_prefix(), "2025-10-14T00:00:00.123Z");
    assert_eq!(s.file_stamp(), "20251014-000000-123");
    assert_eq!(UtcStamp::at(UNIX_EPOCH).log_prefix(), "1970-01-01T00:00:00.000Z");
}

#[test]
fn log_lines_carry_a_utc_prefix_and_track_the_last_write() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("x.log");
    let log = Log::to_file(&path);
    assert!(log.last_write().is_none());
    log.line("hello");
    assert!(log.last_write().is_some());
    log.close();
    log.line("dropped after close");
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    assert_eq!(first.len(), "2026-10-14T00:00:00.000Z hello".len());
    assert!(first.ends_with("Z hello"));
    assert!(lines.next().is_none());
}

#[test]
fn installed_log_names_its_file_by_component_and_prunes_old_ones() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let old = dir.path().join("watchdog-20200101-000000-000.log");
    let other = dir.path().join("updater-20200101-000000-000.log");
    for p in [&old, &other] {
        fs::write(p, b"x").unwrap_or_else(|e| panic!("{e}"));
        let f = fs::File::options()
            .write(true)
            .open(p)
            .unwrap_or_else(|e| panic!("{e}"));
        f.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 3600))
            .unwrap_or_else(|e| panic!("{e}"));
    }

    let log = Log::install(dir.path(), "watchdog");
    log.line("started");
    let current = log.current_path().unwrap_or_default();
    log.close();

    assert!(!old.exists());
    assert!(other.exists());
    let name = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert!(
        name.starts_with("watchdog-")
            && name.ends_with(".log")
            && name.len() == "watchdog-20261014-000000-000.log".len(),
        "{name}"
    );
    assert!(
        fs::read_to_string(&current)
            .unwrap_or_default()
            .ends_with("Z started\n")
    );
}

#[test]
fn prune_keeps_recent_logs_and_unrelated_files() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let recent = dir.path().join("watchdog-20261013-000000-000.log");
    let unrelated = dir.path().join("watchdog.txt");
    fs::write(&recent, b"x").unwrap_or_else(|e| panic!("{e}"));
    fs::write(&unrelated, b"x").unwrap_or_else(|e| panic!("{e}"));
    log::prune(
        dir.path(),
        "watchdog",
        SystemTime::now() + Duration::from_secs(30 * 24 * 3600),
    );
    assert!(!recent.exists());
    assert!(unrelated.exists());
}

#[test]
fn disabled_log_swallows_lines() {
    let log = Log::disabled();
    log.line("nothing");
    assert!(!log.is_enabled());
    assert!(log.current_path().is_none());
}

#[test]
fn crash_report_redacts_the_profile_and_user_name() {
    let r = Report {
        component: "watchdog",
        kind: "panic",
        terminating: true,
        pid: 42,
        version: "2026.10.14.0",
        basedir: r"C:\Users\Alice\AppData\Local\vrcresolver",
        os: "windows",
        snapshot: Some("mesh=up user=ALICE".to_owned()),
        detail: r"panic at c:\users\alice\src\main.rs:1",
    };
    let text = crash::render(&r, UNIX_EPOCH);
    let red = crash::redact(&text, Some(r"C:\Users\Alice"), Some("alice"));
    assert!(red.starts_with("=== vrcresolver crash log ===\ntimestamp:    1970-01-01T00:00:00.0000000Z\n"));
    assert!(red.contains("terminating:  True\n"));
    assert!(red.contains(r"basedir:      %USERPROFILE%\AppData\Local\vrcresolver"));
    assert!(red.contains("--- state snapshot ---\nmesh=up user=<user>\n"));
    assert!(red.contains(r"panic at %USERPROFILE%\src\main.rs:1"));
    assert!(!red.to_ascii_lowercase().contains("alice"));
}

#[test]
fn crash_redaction_skips_short_user_names() {
    assert_eq!(crash::redact("al is here", None, Some("al")), "al is here");
    assert_eq!(crash::redact("x", Some(""), None), "x");
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
    let store = store.map(|s| s as &mut dyn DwordStore);
    wininet::restore(store, m, &mut |l| lines.push(l)).unwrap_or_else(|e| panic!("{e}"));
    lines
}

#[test]
fn wininet_decide_raise_only_lifts_values_below_target() {
    assert_eq!(wininet::decide_raise(None), Some(16));
    assert_eq!(wininet::decide_raise(Some(2)), Some(16));
    assert_eq!(wininet::decide_raise(Some(16)), None);
    assert_eq!(wininet::decide_raise(Some(32)), None);
}

#[test]
fn wininet_ensure_then_restore_puts_an_absent_value_back_to_absent() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake::default();

    let lines = run_ensure(&mut store, &m);
    assert_eq!(store.value, Some(16));
    assert_eq!(fs::read_to_string(&m).unwrap_or_default(), "absent");
    assert!(lines[0].starts_with("raised WinINet connections per server from default 2 to 16"));

    let lines = run_restore(Some(&mut store), &m);
    assert_eq!(store.value, None);
    assert_eq!(lines, ["removed WinINet connections per server override"]);
    assert!(!m.exists());
}

#[test]
fn wininet_restore_puts_a_previous_value_back_and_keeps_the_first_marker() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake { value: Some(4) };
    run_ensure(&mut store, &m);
    store.value = Some(8);
    run_ensure(&mut store, &m);
    assert_eq!(fs::read_to_string(&m).unwrap_or_default(), "4");

    let lines = run_restore(Some(&mut store), &m);
    assert_eq!(store.value, Some(4));
    assert_eq!(lines, ["restored WinINet connections per server to 4"]);
}

#[test]
fn wininet_leaves_a_user_change_alone_on_restore() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake { value: Some(4) };
    run_ensure(&mut store, &m);
    store.value = Some(24);
    assert!(run_restore(Some(&mut store), &m).is_empty());
    assert_eq!(store.value, Some(24));
    assert!(!m.exists());
}

#[test]
fn wininet_ensure_is_a_no_op_at_or_above_target() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake { value: Some(16) };
    assert!(run_ensure(&mut store, &m).is_empty());
    assert!(!m.exists());
}

#[test]
fn wininet_restore_without_a_key_drops_the_marker() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    fs::create_dir_all(dir.path().join("state")).unwrap_or_else(|e| panic!("{e}"));
    fs::write(&m, "4").unwrap_or_else(|e| panic!("{e}"));
    assert!(run_restore(None, &m).is_empty());
    assert!(!m.exists());
}
