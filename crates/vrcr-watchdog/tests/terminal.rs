mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::mesh::Capture;
use tempfile::TempDir;
use vrcr_shared::console::{Color, Style, Ux};
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
    self, ASCII, CommandLine, Frame, Run, UNICODE, activity_glyph, pulse_color, sparkline, trim_left,
    trim_right,
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

#[test]
fn trims_keep_the_useful_end() {
    assert_eq!(trim_right("abcdef", 4), "ab..");
    assert_eq!(trim_right("abcdef", 2), "ab");
    assert_eq!(trim_right("abc", 3), "abc");
    assert_eq!(trim_right("abc", 0), "");
    assert_eq!(trim_left("abcdefgh", 6), "...fgh");
    assert_eq!(trim_left("abcdefgh", 3), "fgh");
    assert_eq!(trim_left("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 4), "...\u{e9}");
}

#[test]
fn sparkline_scales_to_the_peak_and_skips_silence() {
    assert_eq!(sparkline(&[0, 0, 0], 8, &ASCII), "");
    assert_eq!(sparkline(&[], 8, &ASCII), "");
    assert_eq!(sparkline(&[0, 50, 100], 8, &ASCII), "_=#");
    assert_eq!(sparkline(&[0, 58, 100], 8, &ASCII), "_##");
    assert_eq!(sparkline(&[1, 2, 3, 4], 2, &UNICODE).chars().count(), 2);
}

#[test]
fn activity_glyph_and_pulse_follow_motion_settings() {
    assert_eq!(activity_glyph(&ASCII, false, 3, true), "");
    assert_eq!(activity_glyph(&ASCII, true, 3, false), "*");
    assert_eq!(activity_glyph(&ASCII, true, 5, true), "/");
    assert_eq!(pulse_color(0, 0, false), Color::DarkGray);
    assert_eq!(pulse_color(1, 2, true), Color::Green);
    assert_eq!(pulse_color(4, 0, true), Color::Gray);
}

#[test]
fn command_line_splits_verb_and_arguments() {
    let c = CommandLine::parse("  /Settings  set hq on ");
    assert_eq!(c.verb, "settings");
    assert_eq!(c.args, "set hq on");
    let c = CommandLine::parse("help");
    assert_eq!(c.verb, "help");
    assert_eq!(c.args, "");
}

#[test]
fn fast_refresh_needs_motion_and_recent_traffic() {
    let mut s = AppSettings::default();
    let active = Activity {
        last_relay: Some(t0() - Duration::from_secs(1)),
        ..Activity::default()
    };
    assert!(text::fast_refresh(&active, t0(), &s, true));
    assert!(!text::fast_refresh(&active, t0(), &s, false));
    assert!(!text::fast_refresh(&idle(), t0(), &s, true));
    let stale = Activity {
        last_relay: Some(t0() - Duration::from_secs(3)),
        ..Activity::default()
    };
    assert!(!text::fast_refresh(&stale, t0(), &s, true));
    s.terminal.animations = false;
    assert!(!text::fast_refresh(&active, t0(), &s, true));
}

#[test]
fn input_buffer_edits_at_the_cursor() {
    let mut b = InputBuffer::new(Vec::new());
    for c in "helo".chars() {
        b.append(c);
    }
    b.move_left();
    b.append('l');
    assert_eq!(b.text(), "hello");
    assert_eq!(b.cursor(), 4);
    b.move_home();
    b.delete();
    assert_eq!(b.text(), "ello");
    b.move_end();
    b.backspace();
    assert_eq!(b.text(), "ell");
    assert!(b.at_end());
    assert_eq!(b.take(), "ell");
    assert_eq!(b.text(), "");
}

#[test]
fn input_buffer_caps_length_and_walks_history() {
    let mut b = InputBuffer::new(vec!["one".to_owned(), " ".to_owned(), "two".to_owned()]);
    b.previous_history();
    assert_eq!(b.text(), "two");
    b.previous_history();
    assert_eq!(b.text(), "one");
    b.previous_history();
    assert_eq!(b.text(), "one");
    b.next_history();
    assert_eq!(b.text(), "two");
    b.next_history();
    assert_eq!(b.text(), "");
    b.remember("two");
    b.previous_history();
    b.previous_history();
    assert_eq!(b.text(), "one");

    let long: String = std::iter::repeat_n('x', MAX_INPUT_CHARS + 10)
        .chain(['y'])
        .collect();
    b.set(&long);
    assert_eq!(b.text().chars().count(), MAX_INPUT_CHARS);
    assert!(b.text().ends_with('y'));
}

#[test]
fn session_store_writes_history_and_events() {
    let dir = tmp();
    let store = SessionStore::new(dir.path(), t0(), fixed_now);
    assert!(
        store
            .session_path()
            .file_name()
            .is_some_and(|n| n.to_string_lossy() == "session-20251009-085320.jsonl")
    );
    store.start();
    store.record_command(" status ");
    store.record_command("status");
    store.record_command("help");
    store.record_output("Warn", "careful");
    store.stop();
    store.stop();

    assert_eq!(store.recent_commands(), vec!["status", "help"]);
    assert_eq!(store.load_history(), vec!["status", "status", "help"]);
    let events = read(store.session_path());
    let lines: Vec<serde_json::Value> = events
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}")))
        .collect();
    let kinds: Vec<&str> = lines.iter().filter_map(|v| v["type"].as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "session_start",
            "command",
            "command",
            "command",
            "output:warn",
            "session_end"
        ]
    );
    assert_eq!(lines[1]["command"], "status");
    assert!(lines[1]["text"].is_null());
    assert_eq!(lines[0]["time_utc"], "2025-10-09T08:53:20.0000000Z");
}

#[test]
fn session_history_ignores_an_oversized_file_and_keeps_the_tail() {
    let dir = tmp();
    let store = SessionStore::new(dir.path(), t0(), fixed_now);
    let many: String = (0..250).map(|i| format!("cmd{i}\n\n")).collect();
    fs::write(store.history_path(), many).unwrap_or_else(|e| panic!("{e}"));
    let history = store.load_history();
    assert_eq!(history.len(), 200);
    assert_eq!(history[0], "cmd50");
    assert_eq!(history[199], "cmd249");

    fs::write(store.history_path(), "x".repeat(300 * 1024)).unwrap_or_else(|e| panic!("{e}"));
    assert!(store.load_history().is_empty());
}

#[test]
fn wrap_breaks_on_spaces_and_hard_splits_long_words() {
    assert_eq!(blocks::wrap("aaa bbb ccc", 8), vec!["aaa bbb", "ccc"]);
    assert_eq!(blocks::wrap("abcdefghijkl", 8), vec!["abcdefgh", "ijkl"]);
    assert!(blocks::wrap("   ", 8).is_empty());
    assert_eq!(blocks::wrap("one\ntwo", 20), vec!["one", "two"]);
}

#[test]
fn table_falls_back_to_key_value_when_narrow() {
    let rows = vec![(
        "status-line".to_owned(),
        "on".to_owned(),
        "Show the live status line.".to_owned(),
    )];
    let wide = blocks::table("settings", &rows, 100, &ASCII);
    assert!(wide[1].plain.contains("setting") && wide[1].plain.contains("about"));
    assert_eq!(wide[2].runs[2].color, Color::Green);
    let narrow = blocks::table("settings", &rows, 30, &ASCII);
    assert!(narrow.iter().any(|f| f.plain.contains("on - Show")));
}

#[test]
fn panel_draws_a_box_and_colors_states() {
    let rows = vec![
        (
            "resolver".to_owned(),
            "online idle".to_owned(),
            "detail".to_owned(),
        ),
        ("mesh".to_owned(), "reconnecting".to_owned(), "x".to_owned()),
    ];
    let frames = blocks::panel("status", &rows, 80, &ASCII);
    assert!(frames[0].plain.starts_with("+ status -"));
    assert!(frames[0].plain.ends_with('+'));
    assert_eq!(frames[0].width(), 80);
    assert!(frames.iter().all(|f| f.width() == 80));
    assert_eq!(frames[1].runs[3].color, Color::Green);
    assert_eq!(frames[2].runs[3].color, Color::Yellow);
    let narrow = blocks::panel("status", &rows, 40, &ASCII);
    assert!(narrow[1].plain.contains("online idle  detail"));
}

#[test]
fn state_colors_match_the_original_rules() {
    assert_eq!(blocks::state_color("pulling media"), Color::Green);
    assert_eq!(blocks::state_color("fallback"), Color::Yellow);
    assert_eq!(blocks::state_color("not eligible"), Color::Red);
    assert_eq!(blocks::state_color("off"), Color::Red);
    assert_eq!(blocks::state_color("managed"), Color::Gray);
}

#[test]
fn status_line_uses_full_runs_when_wide() {
    let active = Activity {
        relay_bytes: 2048,
        last_relay: Some(t0()),
        ..Activity::default()
    };
    let bw = Bandwidth {
        current: 1024,
        peak: 1024,
        history: vec![0, 512, 1024],
    };
    let frame = status::format(&active, &bw, t0(), &line("he", 140));
    assert!(
        frame
            .plain
            .starts_with("VRChat | serving 1.0 KB/s now 2.0 KB served")
    );
    assert!(frame.plain.ends_with(&format!("online  {PROMPT}he")));
    assert_eq!(frame.cursor, Some(frame.width()));
    assert!(frame.plain.contains("_=#"));
}

#[test]
fn status_line_degrades_to_a_fitted_plain_line() {
    let frame = status::format(
        &idle(),
        &no_bandwidth(),
        t0(),
        &line("settings set high-quality", 40),
    );
    assert!(frame.width() <= 40);
    assert!(frame.plain.contains(PROMPT));
    assert!(frame.plain.ends_with("high-quality"));
    assert_eq!(frame.runs.len(), 1);
    assert_eq!(frame.cursor, Some(frame.width()));
}

#[test]
fn prompt_only_mode_shows_ghost_text_and_cursor() {
    let mut l = line("he", 80);
    l.status_line = false;
    l.ghost = "lp ";
    l.cursor = Some(1);
    let frame = status::format(&idle(), &no_bandwidth(), t0(), &l);
    assert_eq!(frame.plain, format!("{PROMPT}help "));
    assert_eq!(frame.runs[1].color, Color::DarkGray);
    assert_eq!(frame.cursor, Some(PROMPT.len() + 1));
    assert_eq!(status::normalize_width(0), 119);
    assert_eq!(status::normalize_width(500), 180);
}

#[test]
fn frame_gate_reuses_frames_and_ignores_the_spinner_when_still() {
    let mut gate = FrameGate::default();
    assert_eq!(gate.last_width(), None);
    let mut l = line("", 120);
    let a = gate.frame(&idle(), &no_bandwidth(), t0(), &l);
    l.spinner = 7;
    let b = gate.frame(&idle(), &no_bandwidth(), t0() + Duration::from_secs(1), &l);
    assert_eq!(a, b);
    assert_eq!(gate.last_width(), Some(120));
    l.input = "x";
    let c = gate.frame(&idle(), &no_bandwidth(), t0(), &l);
    assert_ne!(a, c);
}

#[test]
fn overlay_line_only_writes_changes_and_clears_its_width() {
    let mut out = Vec::new();
    let mut ov = OverlayLine::default();
    let frame = Frame::plain("abc", Color::Gray);
    assert!(
        ov.render_if_changed(&mut out, &frame, false)
            .unwrap_or_else(|e| panic!("{e}"))
    );
    assert!(
        !ov.render_if_changed(&mut out, &frame, false)
            .unwrap_or_else(|e| panic!("{e}"))
    );
    ov.clear(&mut out).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(String::from_utf8_lossy(&out), "abc\r   \r");
    assert!(!ov.visible());

    let mut out = Vec::new();
    park_caret(&mut out, 0).unwrap_or_else(|e| panic!("{e}"));
    park_caret(&mut out, 12).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(String::from_utf8_lossy(&out), "\r\r\x1b[12C");

    let mut out = Vec::new();
    write_runs(&mut out, &[Run::new("a", Color::Green)], true).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(String::from_utf8_lossy(&out), "\x1b[92ma\x1b[0m");
}

#[test]
fn registry_completes_commands_with_and_without_a_slash() {
    let r = Registry::with_defaults();
    assert_eq!(r.complete("/he").replacement, "/help ");
    assert_eq!(r.complete("cl").replacement, "clear ");
    assert_eq!(r.complete("exi").replacement, "quit ");
    let s = r.complete("s");
    assert!(s.replacement.is_empty());
    let names: Vec<&str> = s.suggestions.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(names, vec!["settings", "status"]);
    assert_eq!(r.complete("").suggestions.len(), r.all().len());
    assert!(r.complete("nope ").suggestions.is_empty());
}

#[test]
fn registry_suggests_ghost_suffixes() {
    let r = Registry::with_defaults();
    assert_eq!(r.suggest("/hi"), "story ");
    assert_eq!(r.suggest("HE"), "lp ");
    assert_eq!(r.suggest("he"), "lp ");
    assert_eq!(r.suggest("help "), "");
    assert_eq!(r.suggest("settings set high-q"), "uality ");
    assert_eq!(r.suggest(""), "");
}

#[test]
fn registry_resolves_aliases_and_nearest_matches() {
    let r = Registry::with_defaults();
    assert_eq!(r.get("/STATS").map(|c| c.name.as_str()), Some("status"));
    assert_eq!(r.get("?").map(|c| c.name.as_str()), Some("help"));
    assert!(r.get("").is_none());
    let near: Vec<String> = r.nearest("stauts", 3).into_iter().map(|i| i.text).collect();
    assert_eq!(near[0], "status");
    assert!(r.nearest("zzzzzzzzzz", 3).is_empty());
    assert!(r.nearest("", 3).is_empty());
    assert_eq!(edit_distance("kitten", "sitting"), 3);
    assert_eq!(edit_distance("", "abc"), 3);
}

#[test]
fn settings_completion_walks_actions_names_and_values() {
    let r = Registry::with_defaults();
    let first = r.complete("settings ");
    let texts: Vec<&str> = first.suggestions.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(&texts[..4], &["get", "set", "reset", "help"]);
    assert_eq!(texts.len(), 4 + 7);
    assert_eq!(r.complete("settings ge").replacement, "settings get ");
    assert_eq!(
        r.complete("/settings set hi").replacement,
        "/settings set high-quality "
    );
    assert_eq!(r.complete("settings reset al").replacement, "settings reset all ");
    assert_eq!(
        r.complete("settings set high-quality of").replacement,
        "settings set high-quality off"
    );
    assert_eq!(r.complete("settings set high-quality ").suggestions.len(), 2);
    assert_eq!(r.complete("settings animations o").suggestions.len(), 2);
    assert!(r.complete("settings set bogus o").suggestions.is_empty());
    assert_eq!(r.complete("help st").replacement, "help status");
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
    let dir = tmp();
    let out = Capture::default();
    let ux = Ux::new(Box::new(out.clone()), Style::plain(), String::new);
    let settings = Arc::new(Store::new(dir.path().join("settings.json"), Box::new(|_| {})));
    let stats = Arc::new(Stats::new(t0()));
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let counter = shutdowns.clone();
    let session = Arc::new(SessionStore::new(&dir.path().join("terminal"), t0(), fixed_now));
    let width_fn: fn() -> Option<usize> = if width.is_some() { || Some(100) } else { || None };
    let services = Services {
        ux,
        stats: stats.clone(),
        settings: settings.clone(),
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
    let core = Core::new(services, session.clone());
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

#[test]
fn typing_help_echoes_and_lists_every_command() {
    let h = harness();
    h.run("/help");
    let out = h.out.text();
    assert!(out.contains("vrcr> /help\n"));
    assert!(out.contains("* commands  tab completes a unique match"));
    assert!(out.contains("/settings"));
    assert!(out.contains("/stats, /activity, /dashboard"));
    assert!(out.contains("[terminal] tab completes as you type; help <command> explains one."));
    assert_eq!(h.session.recent_commands(), vec!["/help"]);
}

#[test]
fn help_for_one_command_and_unknown_commands() {
    let h = harness();
    h.run("help settings");
    let out = h.out.text();
    assert!(out.contains("* help"));
    assert!(out.contains("config"));
    assert!(out.contains("tab completes them"));
    h.run("stauts");
    let out = h.out.text();
    assert!(out.contains("[terminal][warn] unknown command: stauts"));
    assert!(out.contains("* matches"));
    h.run("zzzzzzzzzz");
    assert!(h.out.text().contains("type help for the full list."));
}

#[test]
fn settings_commands_change_and_reset_the_store() {
    let h = harness();
    h.run("settings set high-quality on");
    assert!(h.settings.snapshot().playback.high_quality);
    assert!(h.out.text().contains("settings: high-quality = on"));
    h.run("settings update-check off");
    assert!(
        h.out
            .text()
            .contains("settings: update-check = off (takes effect next launch)")
    );
    h.run("settings set hq maybe");
    assert!(h.out.text().contains("invalid value for hq:"));
    h.run("settings reset all");
    assert!(!h.settings.snapshot().playback.high_quality);
    assert!(h.settings.snapshot().maintenance.update_check);
    h.run("settings get bogus");
    assert!(
        h.out
            .text()
            .contains("unknown setting: bogus (use settings to list names)")
    );
    h.run("settings get");
    assert!(h.out.text().contains("usage: settings get <name>"));
    h.run("settings set x");
    assert!(h.out.text().contains("usage: settings set <name> <value>"));
    h.run("settings reset");
    assert!(h.out.text().contains("usage: settings reset <name|all>"));
    h.run("settings animations");
    assert!(h.out.text().contains("* setting"));
    h.run("settings");
    assert!(h.out.text().contains("secure-local-video"));
    h.run("settings help");
    assert!(h.out.text().contains("settings status-line off"));
}

#[test]
fn quit_and_ctrl_d_request_shutdown() {
    let h = harness();
    h.typed("x");
    h.core.handle_key(Key::CtrlD);
    assert_eq!(h.shutdowns.load(Ordering::SeqCst), 0);
    h.core.handle_key(Key::CtrlU);
    h.core.handle_key(Key::CtrlD);
    assert_eq!(h.shutdowns.load(Ordering::SeqCst), 1);
    h.run("exit");
    assert_eq!(h.shutdowns.load(Ordering::SeqCst), 2);
    assert!(h.out.text().contains("quit requested; shutting down."));
}

#[test]
fn tab_completes_and_right_accepts_the_ghost() {
    let h = harness();
    h.typed("se");
    assert_eq!(h.core.ghost(), "ttings ");
    h.core.handle_key(Key::Right);
    assert_eq!(h.core.input_text(), "settings ");
    h.core.handle_key(Key::Escape);
    h.typed("s");
    h.core.handle_key(Key::Tab);
    assert_eq!(h.core.input_text(), "s");
    assert!(h.out.text().contains("vrcr> s\n* matches"));
    h.core.handle_key(Key::Left);
    assert_eq!(h.core.ghost(), "");
    h.core.handle_key(Key::End);
    h.typed("t");
    h.core.handle_key(Key::Tab);
    assert_eq!(h.core.input_text(), "status ");
}

#[test]
fn up_arrow_recalls_submitted_commands() {
    let h = harness();
    h.run("history");
    assert!(h.out.text().contains(" 1  history"));
    h.core.handle_key(Key::Up);
    assert_eq!(h.core.input_text(), "history");
    h.core.handle_key(Key::Down);
    assert_eq!(h.core.input_text(), "");
    h.core.handle_key(Key::Enter);
    assert_eq!(h.session.recent_commands(), vec!["history"]);
}

#[test]
fn ctrl_c_cancels_input_with_a_warning() {
    let h = harness();
    h.typed("abc");
    h.core.handle_key(Key::CtrlC);
    assert_eq!(h.core.input_text(), "");
    assert!(h.out.text().contains("input cancelled."));
}

#[test]
fn status_tools_diagnostics_and_update_render() {
    let h = harness_with(Some(100));
    h.stats.record_relay_bytes(false, 4096, t0());
    h.run("status");
    let out = h.out.text();
    assert!(out.contains("+ status "));
    assert!(out.contains("serving VRChat"));
    assert!(out.contains("4.0 KB served"));
    h.run("tools");
    assert!(h.out.text().contains("resolver mesh"));
    h.run("diag");
    let out = h.out.text();
    assert!(out.contains("* diagnostics"));
    assert!(out.contains("this session"));
    h.run("permissions");
    assert!(h.out.text().contains("shell execution is not exposed."));
    h.run("update");
    assert!(
        h.out
            .text()
            .contains("[terminal][err] updater not found in install folder")
    );
}

#[test]
fn render_writes_the_status_line_through_the_overlay() {
    let h = harness();
    let guard = h.core.renderer().attach();
    h.typed("he");
    let out = h.out.text();
    assert!(out.contains("VRChat waiting 0 B served  resolver idle online  vrcr> help "));
    drop(guard);
    let before = h.out.text().len();
    h.core.renderer().render_overlay();
    assert_eq!(h.out.text().len(), before);
}

#[test]
fn key_decoder_maps_keys_ctrl_and_surrogates() {
    let ev = |vk: u16, ch: u16, ctrl: bool| KeyEvent { vk, ch, ctrl };
    let mut d = KeyDecoder::default();
    assert_eq!(d.decode(ev(0x0D, 13, false)), Key::Enter);
    assert_eq!(d.decode(ev(0x0D, 13, true)), Key::Enter);
    assert_eq!(d.decode(ev(0x55, 0x15, true)), Key::CtrlU);
    assert_eq!(d.decode(ev(0x41, 0x01, true)), Key::Ignored);
    assert_eq!(d.decode(ev(0x51, u16::from(b'@'), true)), Key::Char('@'));
    assert_eq!(d.decode(ev(0x41, u16::from(b'a'), false)), Key::Char('a'));
    assert_eq!(d.decode(ev(0, 0xD83D, false)), Key::Ignored);
    assert_eq!(d.decode(ev(0, 0xDE00, false)), Key::Char('\u{1F600}'));
    assert_eq!(d.decode(ev(0, 0xDE00, false)), Key::Ignored);
    assert_eq!(d.decode(ev(0x26, 0, false)), Key::Up);
}

#[test]
fn interactive_and_animation_gates_follow_the_environment() {
    assert!(interactive_allowed(true, true, None));
    assert!(!interactive_allowed(true, false, None));
    assert!(!interactive_allowed(true, true, Some("TRUE")));
    assert!(interactive_allowed(true, true, Some("0")));
    assert!(animations_allowed(true, true, None));
    assert!(!animations_allowed(true, false, None));
    assert!(!animations_allowed(true, true, Some("1")));
}

#[test]
fn widths_and_ages_match_the_original_formatting() {
    assert_eq!(console_width(None), 119);
    assert_eq!(console_width(Some(10)), 20);
    assert_eq!(console_width(Some(121)), 120);
    assert_eq!(usable_width(120), 118);
    assert_eq!(usable_width(5), 20);
    assert_eq!(describe_age(None, t0()), "not yet");
    assert_eq!(describe_age(Some(t0() + Duration::from_secs(5)), t0()), "now");
    assert_eq!(
        describe_age(Some(t0() - Duration::from_secs(59)), t0()),
        "59s ago"
    );
    assert_eq!(describe_age(Some(t0() - Duration::from_secs(61)), t0()), "1m ago");
    assert_eq!(
        describe_age(Some(t0() - Duration::from_secs(500 * 3600)), t0()),
        "99h ago"
    );
    let mut s = AppSettings::default();
    assert_eq!(describe_terminal(&s), "status line on, motion on");
    s.terminal.animations = false;
    assert_eq!(describe_terminal(&s), "status line on, motion off");
    s.terminal.status_line = false;
    assert_eq!(describe_terminal(&s), "status line off");
}

#[test]
fn status_rows_describe_idle_and_disconnected_states() {
    let rows = status_rows(
        &idle(),
        &no_bandwidth(),
        false,
        &AppSettings::default(),
        t0(),
        &ASCII,
    );
    let find = |n: &str| rows.iter().find(|r| r.0 == n).cloned().unwrap_or_default();
    assert_eq!(find("local video").1, "waiting");
    assert_eq!(find("local video").2, "0 B served total");
    assert_eq!(find("resolver").1, "reconnecting");
    assert_eq!(find("resolver").2, "no media pulled yet");
    assert_eq!(find("peak speed").1, "none yet");
    assert_eq!(find("resolves").2, "0 local video, 0 cache hits");
    assert_eq!(rows.len(), 8);
}

#[test]
fn completion_palette_and_items_render() {
    let frames = blocks::completion_palette(&[Item::new("status", "Show status.")], 60, &ASCII);
    assert_eq!(frames[0].plain, "* matches");
    assert!(frames[1].plain.starts_with("  - status"));
}
