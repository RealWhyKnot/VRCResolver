mod common;

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::win::Elevated;
use vrcr_watchdog::hosts::{
    self, ADD_ARG, HostsControl, HostsManager, HostsTicker, REMOVE_ARG, line_is_bypass_entry,
};

use common::mesh::Capture;

const GIVE_UP: &str = "won't ask again";

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (Ux::new(Box::new(out.clone()), Style::plain(), String::new), out)
}

#[test]
fn bypass_line_table() {
    let cases: &[(&str, bool)] = &[
        ("127.0.0.1 localhost.youtube.com", true),
        ("127.0.0.1\tlocalhost.youtube.com", true),
        ("127.0.0.1   localhost.youtube.com   # vrcresolver", true),
        ("127.0.0.1   localhost.youtube.com   # WKVRCProxy", true),
        ("127.0.0.1 localhost.youtube.com  # any other comment", true),
        ("    127.0.0.1 localhost.youtube.com", true),
        ("127.0.0.1 some.other.host localhost.youtube.com extra.host", true),
        ("127.0.0.1 LocalHost.YouTube.com", true),
        ("127.0.0.1 localhost.youtube.com#WKVRCProxy", true),
        ("127.0.0.1 localhost.youtube.com\r", false),
        ("", false),
        ("   ", false),
        ("\t\t", false),
        ("# 127.0.0.1 localhost.youtube.com", false),
        ("    # 127.0.0.1 localhost.youtube.com", false),
        ("#127.0.0.1 localhost.youtube.com", false),
        ("127.0.0.2 localhost.youtube.com", false),
        ("0.0.0.0 localhost.youtube.com", false),
        ("192.168.1.1 localhost.youtube.com", false),
        ("::1 localhost.youtube.com", false),
        ("127.0.0.1 notlocalhost.youtube.com", false),
        ("127.0.0.1 localhost.youtube.com.evil.com", false),
        ("127.0.0.1 prefixlocalhost.youtube.com", false),
        ("127.0.0.1", false),
        ("localhost.youtube.com", false),
        ("127.0.0.1 example.com", false),
        ("# WKVRCProxy hosts entry follows", false),
        ("127.0.0.1 #localhost.youtube.com", false),
    ];
    for (line, expected) in cases {
        assert_eq!(line_is_bypass_entry(line), *expected, "{line:?}");
    }
    assert!(hosts::contains_bypass_entry(
        "# hosts\r\n127.0.0.1 localhost\r\n127.0.0.1 localhost.youtube.com\r\n"
    ));
}

fn hosts_file(dir: &TempDir, body: &str) -> std::path::PathBuf {
    let path = dir.path().join("hosts");
    fs::write(&path, body).unwrap_or_else(|e| panic!("{e}"));
    path
}

#[test]
fn add_child_appends_once_with_crlf() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = hosts_file(&dir, "# hosts\r\n127.0.0.1 localhost");
    let (ux, out) = capture_ux();
    assert_eq!(hosts::read_bypass_state(&path), Ok(false));
    assert_eq!(hosts::run_add(&path, &ux), 0);
    assert_eq!(hosts::run_add(&path, &ux), 0);
    let text = fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        text,
        "# hosts\r\n127.0.0.1 localhost\r\n127.0.0.1 localhost.youtube.com # VRCResolver\r\n"
    );
    assert_eq!(hosts::read_bypass_state(&path), Ok(true));
    assert!(out.text().is_empty());

    let missing = dir.path().join("etc").join("hosts");
    assert_eq!(hosts::read_bypass_state(&missing), Ok(false));
    assert_eq!(hosts::run_add(&missing, &ux), 1);
    assert!(
        out.text().contains("[hosts]") && out.text().contains("add failed: "),
        "{}",
        out.text()
    );
}

#[test]
fn remove_child_drops_every_entry_and_keeps_the_rest() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let original = "# header\r\n127.0.0.1 localhost.youtube.com # WKVRCProxy\r\n10.0.0.1 nas\r\n# 127.0.0.1 localhost.youtube.com\r\n127.0.0.1 x localhost.youtube.com\r\n";
    let path = hosts_file(&dir, original);
    let (ux, out) = capture_ux();
    assert_eq!(hosts::run_remove(&path, &ux), 0);
    assert_eq!(
        fs::read_to_string(&path).unwrap_or_default(),
        "# header\r\n10.0.0.1 nas\r\n# 127.0.0.1 localhost.youtube.com\r\n"
    );
    let before = fs::metadata(&path).and_then(|m| m.modified()).ok();
    assert_eq!(hosts::run_remove(&path, &ux), 0);
    assert_eq!(fs::metadata(&path).and_then(|m| m.modified()).ok(), before);
    assert_eq!(hosts::run_remove(&dir.path().join("absent"), &ux), 0);
    assert!(out.text().is_empty());

    let lf = hosts_file(&dir, "a\n127.0.0.1 localhost.youtube.com\nb\n");
    assert_eq!(hosts::run_remove(&lf, &ux), 0);
    assert_eq!(fs::read_to_string(&lf).unwrap_or_default(), "a\r\nb\r\n");
}

#[test]
fn unreadable_hosts_file_reports_the_error() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let err = hosts::read_bypass_state(dir.path()).err().unwrap_or_default();
    assert!(!err.is_empty());
}

struct Script {
    active: Arc<AtomicBool>,
    args: Arc<Mutex<Vec<String>>>,
}

fn manager(initially: bool, outcome: Elevated, adds_entry: bool) -> (HostsManager, Script, Capture) {
    let active = Arc::new(AtomicBool::new(initially));
    let args = Arc::new(Mutex::new(Vec::new()));
    let (ux, out) = capture_ux();
    let read_flag = active.clone();
    let elevate_flag = active.clone();
    let seen = args.clone();
    let m = HostsManager::new(
        Box::new(move || Ok(read_flag.load(Ordering::SeqCst))),
        Box::new(move |arg| {
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(arg.to_owned());
            if adds_entry {
                elevate_flag.store(arg == ADD_ARG, Ordering::SeqCst);
            }
            outcome.clone()
        }),
        ux,
    );
    (m, Script { active, args }, out)
}

impl Script {
    fn args(&self) -> Vec<String> {
        self.args.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[test]
fn declined_elevation_is_recorded_for_the_session() {
    let (m, s, out) = manager(false, Elevated::Declined, false);
    m.ensure_entry();
    assert_eq!(s.args(), vec![ADD_ARG]);
    assert!(m.declined_this_session());
    assert!(!m.prompt_in_flight());
    let text = out.text();
    assert!(text.contains("adding entry for public-instance support -- UAC prompt incoming."));
    assert!(text.contains("UAC declined -- entry not modified."));
    assert!(!text.contains("entry not present after elevation"));
}

#[test]
fn prompt_that_leaves_no_entry_is_a_decline() {
    for outcome in [Elevated::Exited(0), Elevated::TimedOut] {
        let timed_out = outcome == Elevated::TimedOut;
        let (m, _, out) = manager(false, outcome, false);
        m.ensure_entry();
        assert!(m.declined_this_session());
        let text = out.text();
        assert!(text.contains("entry not present after elevation -- public-instance support may not work."));
        assert_eq!(
            text.contains("elevation child still running after 60s -- continuing without hosts entry."),
            timed_out
        );
    }
    let (m, _, out) = manager(false, Elevated::Failed("no shell".to_owned()), false);
    m.ensure_entry();
    assert!(m.declined_this_session());
    assert!(out.text().contains("elevation error: no shell"));
    assert!(!out.text().contains("entry not present"));
}

#[test]
fn prompt_that_adds_the_entry_is_not_a_decline() {
    let (m, s, out) = manager(false, Elevated::Exited(0), true);
    m.ensure_entry();
    assert!(!m.declined_this_session());
    assert!(s.active.load(Ordering::SeqCst));
    assert!(out.text().contains("added 127.0.0.1 localhost.youtube.com"));
}

#[test]
fn existing_entry_never_prompts() {
    let (m, s, out) = manager(true, Elevated::Exited(0), true);
    m.ensure_entry();
    assert!(s.args().is_empty());
    assert!(!m.declined_this_session());
    assert!(out.text().is_empty());
}

#[test]
fn remove_only_elevates_when_the_entry_exists() {
    let (m, s, _) = manager(false, Elevated::Exited(0), true);
    m.remove_entry_if_present();
    assert!(s.args().is_empty());
    s.active.store(true, Ordering::SeqCst);
    m.remove_entry_if_present();
    assert_eq!(s.args(), vec![REMOVE_ARG]);
    assert!(!s.active.load(Ordering::SeqCst));
}

#[test]
fn concurrent_callers_share_one_prompt() {
    let active = Arc::new(AtomicBool::new(false));
    let prompts = Arc::new(AtomicU32::new(0));
    let (ux, _) = capture_ux();
    let (read_flag, flag, count) = (active.clone(), active.clone(), prompts.clone());
    let m = Arc::new(HostsManager::new(
        Box::new(move || Ok(read_flag.load(Ordering::SeqCst))),
        Box::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::SeqCst);
            Elevated::Exited(0)
        }),
        ux,
    ));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let m = m.clone();
            std::thread::spawn(move || m.ensure_entry())
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
    assert!(!m.prompt_in_flight());
}

type OnEnsure = Option<Box<dyn Fn(&Harness) -> Result<(), String> + Send>>;

#[derive(Default)]
struct Harness {
    present: AtomicBool,
    active: AtomicBool,
    in_flight: AtomicBool,
    declined: AtomicBool,
    unreadable: AtomicBool,
    ensure_calls: AtomicU32,
    on_ensure: Mutex<OnEnsure>,
    now: Mutex<u64>,
}

impl Harness {
    fn calls(&self) -> u32 {
        self.ensure_calls.load(Ordering::SeqCst)
    }

    fn set_on_ensure(&self, f: OnEnsure) {
        *self.on_ensure.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }

    fn advance(&self, secs: u64) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) += secs;
    }

    fn set(flag: &AtomicBool, v: bool) {
        flag.store(v, Ordering::SeqCst);
    }
}

impl HostsControl for Harness {
    fn read_state(&self) -> Result<bool, String> {
        if self.unreadable.load(Ordering::SeqCst) {
            return Err("PermissionDenied: denied".to_owned());
        }
        Ok(self.present.load(Ordering::SeqCst))
    }

    fn ensure_entry(&self) -> Result<(), String> {
        self.ensure_calls.fetch_add(1, Ordering::SeqCst);
        self.advance(1);
        match &*self.on_ensure.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(f) => f(self),
            None => Ok(()),
        }
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    fn prompt_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
    }

    fn declined_this_session(&self) -> bool {
        self.declined.load(Ordering::SeqCst)
    }
}

fn ticker(h: &Arc<Harness>) -> (HostsTicker, Capture) {
    let (ux, out) = capture_ux();
    let clock = h.clone();
    let now: vrcr_watchdog::clock::Now = Arc::new(move || {
        SystemTime::UNIX_EPOCH + Duration::from_secs(*clock.now.lock().unwrap_or_else(|e| e.into_inner()))
    });
    (HostsTicker::new(h.clone(), now, ux, Log::disabled()), out)
}

fn give_up_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.contains(GIVE_UP))
        .map(str::to_owned)
        .collect()
}

fn assert_single_ascii_give_up(text: &str) {
    let lines = give_up_lines(text);
    assert_eq!(lines.len(), 1, "{text}");
    let message = &lines[0][lines[0].find("tick:").unwrap_or_default()..];
    assert!(message.is_ascii(), "{message}");
}

#[test]
fn ticker_does_not_reprompt_after_a_decline() {
    let h = Arc::new(Harness::default());
    let (t, out) = ticker(&h);
    t.tick();
    h.advance(3600);
    t.tick();
    h.advance(3600);
    t.tick();
    assert_eq!(h.calls(), 1);
    let text = out.text();
    assert_single_ascii_give_up(&text);
    assert!(text.contains("tick: re-add failed (UAC declined or write blocked)"));
    assert!(!text.contains("next attempt in"));
}

#[test]
fn ticker_waits_while_the_startup_prompt_is_open() {
    let h = Arc::new(Harness::default());
    Harness::set(&h.in_flight, true);
    let (t, out) = ticker(&h);
    t.tick();
    h.advance(60);
    t.tick();
    assert_eq!(h.calls(), 0);
    assert!(!out.text().contains("failed"));
    assert!(!out.text().contains(GIVE_UP));

    Harness::set(&h.in_flight, false);
    h.set_on_ensure(Some(Box::new(|h| {
        Harness::set(&h.active, true);
        Harness::set(&h.present, true);
        Ok(())
    })));
    t.tick();
    assert_eq!(h.calls(), 1);
    assert!(
        out.text().contains("tick: re-add succeeded in 1000 ms"),
        "{}",
        out.text()
    );
}

#[test]
fn declined_startup_prompt_is_not_followed_by_another() {
    let h = Arc::new(Harness::default());
    Harness::set(&h.in_flight, true);
    let (t, out) = ticker(&h);
    t.tick();
    assert!(!out.text().contains(GIVE_UP));
    Harness::set(&h.in_flight, false);
    Harness::set(&h.declined, true);
    t.tick();
    h.advance(3600);
    t.tick();
    t.tick();
    assert_eq!(h.calls(), 0);
    assert_single_ascii_give_up(&out.text());
    assert!(!out.text().contains("re-adding"));
}

#[test]
fn prompt_taken_over_by_another_caller_is_not_a_failed_attempt() {
    let h = Arc::new(Harness::default());
    let (t, out) = ticker(&h);
    h.set_on_ensure(Some(Box::new(|h| {
        Harness::set(&h.in_flight, true);
        Ok(())
    })));
    t.tick();
    assert_eq!(h.calls(), 1);
    assert!(!out.text().contains(GIVE_UP));
    Harness::set(&h.in_flight, false);
    h.set_on_ensure(None);
    t.tick();
    assert_eq!(h.calls(), 2);
    assert!(out.text().contains(GIVE_UP));
}

#[test]
fn successful_readd_is_not_repeated() {
    let h = Arc::new(Harness::default());
    let (t, out) = ticker(&h);
    h.set_on_ensure(Some(Box::new(|h| {
        Harness::set(&h.active, true);
        Harness::set(&h.present, true);
        Ok(())
    })));
    t.tick();
    h.advance(3600);
    t.tick();
    assert_eq!(h.calls(), 1);
    let text = out.text();
    assert!(text.contains("tick: localhost.youtube.com missing -- re-adding"));
    assert!(text.contains("re-add succeeded"));
    assert_eq!(text.matches("entry present").count(), 0);
    assert!(!text.contains(GIVE_UP));
}

#[test]
fn readd_that_errors_counts_as_the_failed_attempt() {
    let h = Arc::new(Harness::default());
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log_path = dir.path().join("w.log");
    let (ux, out) = capture_ux();
    let log = Log::to_file(&log_path);
    let t = HostsTicker::new(h.clone(), vrcr_watchdog::clock::system(), ux, log.clone());
    h.set_on_ensure(Some(Box::new(|_| Err("boom".to_owned()))));
    t.tick();
    t.tick();
    log.close();
    assert_eq!(h.calls(), 1);
    assert_single_ascii_give_up(&out.text());
    assert!(out.text().contains("tick: re-add failed (boom)"));
    assert!(!out.text().contains("re-add threw"));
    assert!(
        fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains("[hosts] re-add threw: boom")
    );
}

#[test]
fn present_and_unreadable_states_log_only_on_change() {
    let h = Arc::new(Harness::default());
    Harness::set(&h.present, true);
    let (t, out) = ticker(&h);
    t.tick();
    t.tick();
    Harness::set(&h.unreadable, true);
    t.tick();
    t.tick();
    Harness::set(&h.unreadable, false);
    t.tick();
    let text = out.text();
    assert_eq!(
        text.matches("tick: localhost.youtube.com entry present").count(),
        2,
        "{text}"
    );
    assert_eq!(
        text.matches("tick: hosts file unreadable (PermissionDenied: denied) -- skipping check")
            .count(),
        1,
        "{text}"
    );
    assert_eq!(h.calls(), 0);

    let fresh = Arc::new(Harness::default());
    Harness::set(&fresh.unreadable, true);
    let (t, out) = ticker(&fresh);
    t.tick();
    assert!(out.text().is_empty());
}

#[test]
fn ticker_thread_ticks_on_its_interval_and_stops_promptly() {
    let h = Arc::new(Harness::default());
    Harness::set(&h.in_flight, true);
    let (t, _) = ticker(&h);
    let handle = Arc::new(t).start(Duration::from_millis(20));
    Harness::set(&h.in_flight, false);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while h.calls() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.calls(), 1);
    assert!(handle.is_running());
    let started = std::time::Instant::now();
    handle.stop();
    assert!(started.elapsed() < Duration::from_secs(1));

    let slow = Arc::new(Harness::default());
    Harness::set(&slow.present, true);
    let (t, _) = ticker(&slow);
    let handle = Arc::new(t).start(Duration::from_secs(3600));
    let started = std::time::Instant::now();
    drop(handle);
    assert!(started.elapsed() < Duration::from_secs(1));
}
