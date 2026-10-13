mod common;

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use common::ManualClock;
use common::mesh::Capture;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::iso8601_utc;
use vrcr_shared::trust_gateway;
use vrcr_shared::win::DefaultOutput;
use vrcr_shared::wire::{ResolveResponse, action, feedback};
use vrcr_watchdog::display::{format_bytes, format_bytes_per_second};
use vrcr_watchdog::health::{self, HealthGate};
use vrcr_watchdog::ipc::Feedback;
use vrcr_watchdog::monitoring::audio::{self, AudioCheck, AudioState};
use vrcr_watchdog::monitoring::heartbeat::{self, Heartbeat};
use vrcr_watchdog::monitoring::vrclog::{
    self, MonitorDeps, PlaybackSink, RelayWedge, VrcLogMonitor, initial_read_offset_for_new_file,
    parse_observed_resolution, should_rescan_directory,
};
use vrcr_watchdog::og_hint::{self, OgFallbackHint};
use vrcr_watchdog::relay::RelayActivity;
use vrcr_watchdog::resolve_cache::{Key, ResolveCache};
use vrcr_watchdog::stats::{Activity, Stats};

const T0: u64 = common::T_2026_09_29;
const PLAYBACK: &str = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";
const SOURCE: &str = "https://virtualfilm.institute/watch?v=abc";

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn ms(m: u64) -> Duration {
    Duration::from_millis(m)
}

#[derive(Default)]
struct Sink(Mutex<Vec<Feedback>>);

impl PlaybackSink for Sink {
    fn send_playback_feedback(&self, feedback: Feedback) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(feedback);
    }
}

impl Sink {
    fn all(&self) -> Vec<(String, &'static str, i32, Option<i32>)> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|f| (f.url.clone(), f.kind, f.ms_since_open, f.delivered_height))
            .collect()
    }

    fn kinds(&self) -> Vec<&'static str> {
        self.all().into_iter().map(|f| f.1).collect()
    }
}

struct Rig {
    clock: ManualClock,
    sink: Arc<Sink>,
    out: Capture,
    dir: TempDir,
    cache: Option<Arc<ResolveCache>>,
    hint: Option<Arc<OgFallbackHint>>,
    gate: Option<Arc<HealthGate>>,
    probes: Arc<AtomicU32>,
    log_path: PathBuf,
    log: Log,
}

impl Rig {
    fn new() -> Rig {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let log_path = dir.path().join("watchdog.log");
        Rig {
            clock: ManualClock::utc(T0),
            sink: Arc::new(Sink::default()),
            out: Capture::default(),
            log: Log::to_file(&log_path),
            log_path,
            dir,
            cache: None,
            hint: None,
            gate: None,
            probes: Arc::new(AtomicU32::new(0)),
        }
    }

    fn with_cache(mut self) -> Rig {
        self.cache = Some(Arc::new(ResolveCache::new(
            self.dir.path().join("resolve_cache.json"),
            self.clock.now(),
            Log::disabled(),
        )));
        self.hint = Some(Arc::new(OgFallbackHint::new(
            og_hint::DEFAULT_TTL,
            self.clock.now(),
        )));
        self
    }

    fn with_gate(mut self) -> Rig {
        self.gate = Some(Arc::new(HealthGate::new(
            health::DEFAULT_COOLDOWN,
            health::DEFAULT_VERDICT_TIMEOUT,
            self.clock.now(),
        )));
        self
    }

    fn deps(&self, audio: AudioCheck) -> MonitorDeps {
        let mut d = MonitorDeps::new(
            self.sink.clone(),
            Arc::new(RwLock::new(FirstPartyPolicy::default())),
            Ux::new(Box::new(self.out.clone()), Style::plain(), String::new),
            self.log.clone(),
        );
        d.cache = self.cache.clone();
        d.og_hint = self.hint.clone();
        d.health = self.gate.clone();
        d.silent_stall_window = secs(300);
        let probes = self.probes.clone();
        d.audio = Box::new(move || {
            probes.fetch_add(1, Ordering::SeqCst);
            audio.clone()
        });
        d
    }

    fn monitor(&self) -> VrcLogMonitor {
        self.monitor_with(self.deps(AudioCheck::new(AudioState::Ok, Some("Speakers"))))
    }

    fn monitor_with(&self, deps: MonitorDeps) -> VrcLogMonitor {
        VrcLogMonitor::new(deps, self.dir.path().join("VRChat"))
    }

    fn now(&self) -> SystemTime {
        self.clock.get()
    }

    fn feed(&self, m: &mut VrcLogMonitor, text: &str) {
        m.process_text(text, self.now());
    }

    fn tick(&self, m: &mut VrcLogMonitor, d: Duration) {
        self.clock.advance(d);
        m.fire_due(self.now());
    }

    fn gate_open(&self) -> bool {
        self.gate
            .as_ref()
            .is_some_and(|g| g.should_short_circuit(false).0)
    }

    fn store(&self, playback: &str) {
        let Some(cache) = &self.cache else { return };
        let resp = ResolveResponse {
            action: action::RESOLVED.to_owned(),
            id: "ignored-on-store".to_owned(),
            url: Some(playback.to_owned()),
            engine: Some("yt-dlp:no-cookies-default".to_owned()),
            container: Some("mp4".to_owned()),
            video_codec: Some("h264".to_owned()),
            audio_codec: Some("aac".to_owned()),
            protocol: Some("https".to_owned()),
            audio_channels: Some(2),
            expires_at: Some(iso8601_utc(self.now() + secs(3600))),
            ..ResolveResponse::default()
        };
        let key = Key {
            node: "us1.vrcresolver.com",
            url: SOURCE,
            player: Some("avpro"),
            format_arg: None,
            max_height: Some(1080),
        };
        assert!(cache.store(&key, &resp).is_some());
    }

    fn cached(&self, playback: &str) -> bool {
        self.cache
            .as_ref()
            .is_some_and(|c| c.source_url_for_resolved(playback).is_some())
    }

    fn file_log(&self) -> String {
        self.log.close();
        fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

fn opening(url: &str) -> String {
    format!(
        "2026.10.11 10:00:00 Debug      -  [AVProVideo] Opening {url} (offset 0) with API MediaFoundation"
    )
}

#[test]
fn offset_rescan_and_resolution_tables() {
    assert_eq!(initial_read_offset_for_new_file(1234, true), 1234);
    assert_eq!(initial_read_offset_for_new_file(1234, false), 0);
    for (has, since, expected) in [
        (false, 0, true),
        (false, 100, true),
        (true, 1000, false),
        (true, 4999, false),
        (true, 5000, true),
        (true, -1, true),
    ] {
        assert_eq!(should_rescan_directory(has, since), expected, "{has} {since}");
    }
    let cases: &[(&str, Option<(i32, i32)>)] = &[
        (
            "[Always] [Video Playback] Switched to 1920x1080",
            Some((1920, 1080)),
        ),
        (
            "[AVProVideo] PostStateChanged: OpeningToPlaying fwidth=1280 fheight=720",
            Some((1280, 720)),
        ),
        (
            "[Video Playback] switched to resolution 640x360",
            Some((640, 360)),
        ),
        ("Switched to 8x8", None),
        ("Switched to 1920x9999", None),
        ("Switched to 99999x1080", None),
        ("fwidth=1280 only", None),
        ("fheight=720 fwidth=1280", None),
        ("   ", None),
        ("Unswitched to 1920x1080", None),
    ];
    for (line, expected) in cases {
        assert_eq!(parse_observed_resolution(line), *expected, "{line}");
    }
}

#[test]
fn resolution_is_stored_for_the_current_url_and_confirms() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(ms(1500));
    r.feed(&mut m, "[Always] [Video Playback] Switched to 1920x1080\n");
    assert_eq!(m.observed_delivered_height(PLAYBACK, r.now()), Some(1080));
    assert_eq!(
        r.sink.all(),
        vec![(PLAYBACK.to_owned(), feedback::PLAYING, 1500, Some(1080))]
    );
}

#[test]
fn partial_lines_wait_for_the_rest() {
    let r = Rig::new();
    let mut m = r.monitor();
    let first = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=first";
    let second = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=second";
    r.feed(
        &mut m,
        &format!(
            "{}\n2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani",
            opening(first)
        ),
    );
    assert_eq!(m.last_opened_url(), Some(first));
    r.feed(
        &mut m,
        "fest.m3u8?q=second (offset 0) with API MediaFoundation\r\n",
    );
    assert_eq!(m.last_opened_url(), Some(second));
}

#[test]
fn a_multibyte_character_split_across_reads_survives() {
    let r = Rig::new();
    let mut m = r.monitor();
    let line = format!("[AVProVideo] Opening {PLAYBACK}&t=\u{00e9}\n");
    let bytes = line.as_bytes();
    let cut = bytes.len() - 2;
    m.process_bytes(&bytes[..cut], r.now());
    assert_eq!(m.last_opened_url(), None);
    m.process_bytes(&bytes[cut..], r.now());
    assert_eq!(
        m.last_opened_url(),
        Some(format!("{PLAYBACK}&t=\u{00e9}").as_str())
    );
}

struct LogDir {
    path: PathBuf,
    clock: SystemTime,
}

impl LogDir {
    fn new(root: &Path) -> LogDir {
        let path = root.join("VRChat");
        fs::create_dir_all(&path).unwrap_or_else(|e| panic!("{e}"));
        LogDir {
            path,
            clock: SystemTime::UNIX_EPOCH + secs(T0),
        }
    }

    fn write(&mut self, name: &str, content: &str) -> PathBuf {
        let file = self.path.join(name);
        fs::write(&file, content).unwrap_or_else(|e| panic!("{e}"));
        self.touch(&file);
        file
    }

    fn append(&mut self, file: &Path, content: &str) {
        OpenOptions::new()
            .append(true)
            .open(file)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .unwrap_or_else(|e| panic!("{e}"));
        self.touch(file);
    }

    fn touch(&mut self, file: &Path) {
        self.clock += secs(10);
        File::options()
            .write(true)
            .open(file)
            .and_then(|f| f.set_modified(self.clock))
            .unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn poll_waits_for_a_line_the_game_is_still_writing() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut m = r.monitor();
    let url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a";
    let log = logs.write("output_log_a.txt", "2026.10.11 10:00:00 Debug      -  boot\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.append(
        &log,
        "2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani",
    );
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), None);
    logs.append(&log, "fest.m3u8?q=a (offset 0) with API MediaFoundation\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(url));
}

#[test]
fn first_log_is_tailed_from_its_end_and_truncation_restarts() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut m = r.monitor();
    let log = logs.write("output_log_a.txt", &format!("{}\n", opening(PLAYBACK)));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), None);
    let b = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b";
    fs::write(&log, format!("{}\n", opening(b))).unwrap_or_else(|e| panic!("{e}"));
    logs.touch(&log);
    r.clock.advance(secs(1));
    m.poll_once(r.now());
    assert_eq!(
        m.last_opened_url(),
        None,
        "a shrunk file resets without replaying"
    );
    fs::write(&log, "x\n").unwrap_or_else(|e| panic!("{e}"));
    logs.touch(&log);
    r.clock.advance(secs(1));
    m.poll_once(r.now());
    logs.append(&log, &format!("{}\n", opening(b)));
    r.clock.advance(secs(1));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(b));
}

#[test]
fn alternating_logs_do_not_replay_old_lines() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let sessions = Arc::new(AtomicU32::new(0));
    let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
    let counter = sessions.clone();
    deps.on_new_session = Some(Box::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    let mut m = r.monitor_with(deps);
    let (a1, a2, b1) = (
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a1",
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a2",
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b1",
    );
    let step = |m: &mut VrcLogMonitor| {
        r.clock.advance(secs(10));
        m.poll_once(r.now());
    };
    let count = || sessions.load(Ordering::SeqCst);

    let a = logs.write("output_log_a.txt", "2026.10.11 10:00:00 Debug      -  boot\n");
    step(&mut m);
    assert_eq!(count(), 0);
    logs.append(&a, &format!("{}\n", opening(a1)));
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(a1), 0));
    let b = logs.write("output_log_b.txt", &format!("{}\n", opening(b1)));
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(b1), 1));
    logs.touch(&a);
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(b1), 1));
    logs.touch(&b);
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(b1), 1));
    logs.append(&a, &format!("{}\n", opening(a2)));
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(a2), 1));
    logs.touch(&b);
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(a2), 1));
}

#[test]
fn half_written_line_stays_with_its_own_log() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut m = r.monitor();
    let (a1, b1) = (
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a1",
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b1",
    );
    let a = logs.write("output_log_a.txt", "2026.10.11 10:00:00 Debug      -  boot\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.append(
        &a,
        "2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani",
    );
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.write("output_log_b.txt", &format!("{}\n", opening(b1)));
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(b1));
    logs.append(&a, "fest.m3u8?q=a1 (offset 0) with API MediaFoundation\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(a1));
}

#[test]
fn missing_directory_and_non_log_files_are_ignored() {
    let r = Rig::new();
    let mut m = r.monitor();
    m.poll_once(r.now());
    let mut logs = LogDir::new(r.dir.path());
    logs.write("debug.txt", &format!("{}\n", opening(PLAYBACK)));
    logs.write("output_log_x.log", &format!("{}\n", opening(PLAYBACK)));
    fs::create_dir_all(logs.path.join("output_log_dir.txt")).unwrap_or_else(|e| panic!("{e}"));
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    let log = logs.write("OUTPUT_LOG_upper.TXT", "boot\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.append(&log, &format!("{}\n", opening(PLAYBACK)));
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(PLAYBACK));
}

#[test]
fn load_failure_arms_the_og_hint_before_evicting_the_cache() {
    let r = Rig::new().with_cache();
    let mut m = r.monitor();
    r.store(PLAYBACK);
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(ms(2500));
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    let hint = r.hint.as_ref().unwrap_or_else(|| panic!("hint"));
    assert!(hint.should_prefer_og(SOURCE));
    assert!(!r.cached(PLAYBACK));
    assert_eq!(
        r.sink.all(),
        vec![(PLAYBACK.to_owned(), feedback::LOAD_FAILURE, 2500, None)]
    );
    let text = r.out.text();
    assert!(text.contains("load_failure ms=2500 url="), "{text}");
    assert!(text.contains(" evicted=1 og_hint=armed"), "{text}");
    assert!(!m.stall_armed());
}

#[test]
fn a_late_load_failure_is_not_attributed_to_the_old_open() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(ms(10_001));
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    assert!(r.sink.all().is_empty());
    assert!(!r.out.text().contains("load_failure"));
    assert_eq!(m.last_opened_url(), Some(PLAYBACK));
    assert!(!m.stall_armed());
}

#[test]
fn mark_playback_failure_arms_the_og_hint_for_the_stall_path() {
    let r = Rig::new().with_cache();
    let m = r.monitor();
    r.store(PLAYBACK);
    let recovery = m.mark_playback_failure(PLAYBACK);
    assert_eq!(recovery.evicted, 1);
    assert!(recovery.og_hint_armed);
    assert!(r.hint.as_ref().is_some_and(|h| h.should_prefer_og(SOURCE)));
    assert!(!r.cached(PLAYBACK));
}

#[test]
fn repeated_failures_on_our_urls_open_the_gate() {
    let r = Rig::new().with_gate();
    let m = r.monitor();
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");
    assert!(!r.gate_open());
    m.mark_playback_failure("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=c");
    assert!(r.gate_open());
    assert!(r.out.text().contains(
        "resolver paused -- 3 playbacks failed in a row; VRChat's own resolver takes over while things recover"
    ));

    let r = Rig::new().with_gate();
    let m = r.monitor();
    for v in ["a", "b", "c"] {
        m.mark_playback_failure(&format!("https://cdn.example.com/video-{v}.mp4"));
    }
    assert!(!r.gate_open());
}

#[test]
fn accept_line_and_observed_resolution_reset_the_streak() {
    for confirm in [
        "[AVProVideo] Using playback path: MediaFoundation\n",
        "[Always] [Video Playback] Switched to 1920x1080\n",
    ] {
        let r = Rig::new().with_gate();
        let mut m = r.monitor();
        m.mark_playback_failure(PLAYBACK);
        m.mark_playback_failure(PLAYBACK);
        r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n{confirm}"));
        m.mark_playback_failure(PLAYBACK);
        m.mark_playback_failure(PLAYBACK);
        assert!(!r.gate_open(), "{confirm}");
    }
}

#[test]
fn a_closed_gate_announces_recovery() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    for _ in 0..3 {
        m.mark_playback_failure(PLAYBACK);
    }
    assert!(r.gate_open());
    r.clock.advance(health::DEFAULT_COOLDOWN + secs(1));
    let _ = r.gate.as_ref().map(|g| g.should_short_circuit(true));
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Using playback path: MediaFoundation\n"),
    );
    assert!(
        r.out
            .text()
            .contains("resolver restored -- mesh resolving re-enabled"),
        "{}",
        r.out.text()
    );
}

#[test]
fn leaving_the_world_mid_load_is_not_a_failed_play() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");
    r.feed(
        &mut m,
        "2026.09.29 13:23:56 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c (offset 0) with API MediaFoundation\n\
         2026.09.29 13:23:59 Debug      -  [Behaviour] OnLeftRoom\n\
         2026.09.29 13:24:00 Error      -  [AVProVideo] Error: Loading failed.  File not found, codec not supported, video resolution too high or insufficient system resources.\n",
    );
    r.tick(&mut m, secs(400));
    assert!(!r.gate_open());
    assert!(r.sink.all().is_empty());
    assert!(m.next_deadline().is_none());
    assert!(r.file_log().contains(
        "[vrclog] stopped watching https://us1.vrcresolver.com/api/proxy/manifest.mp4 reason=world_change"
    ));
}

#[test]
fn a_stalled_play_in_the_same_world_still_counts() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");
    r.feed(
        &mut m,
        "2026.09.29 13:24:01 Debug      -  [Behaviour] Entering Room: Hazy Glow\n\
         2026.09.29 13:24:16 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c (offset 0) with API MediaFoundation\n",
    );
    assert_eq!(m.next_deadline(), Some(r.now() + secs(300)));
    r.tick(&mut m, secs(299));
    assert!(!r.gate_open());
    r.tick(&mut m, secs(1));
    assert!(r.gate_open());
    assert_eq!(
        r.sink.all(),
        vec![(
            "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c".to_owned(),
            feedback::SILENT_STALL,
            300_000,
            None
        )]
    );
    assert!(r.out.text().contains("silent_stall ms=300000 url="));
}

#[test]
fn progress_lines_disarm_the_stall_watchdog() {
    for line in [
        "Now Playing: something",
        "Load Url: x",
        "[AVProVideo] Using playback path: MF",
    ] {
        let r = Rig::new();
        let mut m = r.monitor();
        r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n{line}\n"));
        assert!(!m.stall_armed(), "{line}");
        r.tick(&mut m, secs(600));
        assert!(r.sink.all().is_empty(), "{line}");
    }
}

#[test]
fn playing_feedback_repeats_while_the_height_is_fresh() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(secs(2));
    r.feed(&mut m, "fwidth=1280 fheight=720\n");
    assert_eq!(m.playing_feedback_url(), Some(PLAYBACK));
    r.tick(&mut m, secs(14));
    assert_eq!(r.sink.kinds().len(), 1);
    r.tick(&mut m, secs(1));
    r.tick(&mut m, secs(15));
    let all = r.sink.all();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all[1],
        (PLAYBACK.to_owned(), feedback::PLAYING, 17_000, Some(720))
    );
    assert_eq!(all[2].2, 32_000);

    r.feed(&mut m, "[AVProVideo] Opening https://cdn.example.com/other.mp4\n");
    assert_eq!(m.playing_feedback_url(), None);
    r.feed(&mut m, "Now Playing: other\n");
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    assert_eq!(
        m.playing_feedback_url(),
        Some(PLAYBACK),
        "a remembered height restarts the loop"
    );
    r.feed(&mut m, "Now Playing: again\n");
    r.tick(&mut m, vrclog::DELIVERED_HEIGHT_TTL);
    assert_eq!(m.playing_feedback_url(), None, "an expired height ends the loop");
    let before = r.sink.all().len();
    r.tick(&mut m, secs(60));
    assert_eq!(r.sink.all().len(), before);
}

#[test]
fn failure_feedback_carries_the_delivered_height() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {PLAYBACK}\nSwitched to 1920x1080\n"),
    );
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Error: Loading failed\n"),
    );
    let all = r.sink.all();
    assert_eq!(
        all.last().map(|f| (f.1, f.3)),
        Some((feedback::LOAD_FAILURE, Some(1080)))
    );
}

#[test]
fn trust_gateway_urls_are_reported_by_their_target() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    let policy = FirstPartyPolicy::default();
    let local = trust_gateway::build(&policy, 51234, PLAYBACK, Some("s1"), "http").unwrap_or_default();
    assert!(local.starts_with("http://localhost.youtube.com:51234/play/s1/manifest"));
    assert_eq!(m.canonical(&local), PLAYBACK);
    assert_eq!(
        m.canonical("https://cdn.example.com/a.mp4"),
        "https://cdn.example.com/a.mp4"
    );
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {local}\n[AVProVideo] Error: Loading failed\n"),
    );
    assert_eq!(r.sink.all()[0].0, PLAYBACK);
}

#[test]
fn a_wedged_relay_is_handed_to_the_bypass_instead_of_counting() {
    for handled in [true, false] {
        let r = Rig::new().with_gate();
        let activity = Arc::new(RelayActivity::new());
        let ports = Arc::new(Mutex::new(Vec::new()));
        let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
        let seen = ports.clone();
        deps.relay_wedge = Some(RelayWedge {
            activity: activity.clone(),
            on_wedged: Box::new(move |p| {
                seen.lock().unwrap_or_else(|e| e.into_inner()).push(p);
                handled
            }),
        });
        let mut m = r.monitor_with(deps);
        let local = format!("http://localhost.youtube.com:51234/play/p{handled}/manifest.m3u8?target=x");
        m.mark_playback_failure(PLAYBACK);
        m.mark_playback_failure(PLAYBACK);
        r.feed(&mut m, &format!("[AVProVideo] Opening {local}\n"));
        r.tick(&mut m, secs(300));
        assert_eq!(*ports.lock().unwrap_or_else(|e| e.into_inner()), vec![51234]);
        let text = r.out.text();
        assert!(
            text.contains("relay_pool_wedged ms=300000 port=51234: VRChat never reached the local relay")
        );
        assert_eq!(text.contains("; counting as a failed play"), !handled);
        assert_eq!(
            r.sink.kinds(),
            if handled {
                vec![]
            } else {
                vec![feedback::SILENT_STALL]
            }
        );

        activity.record(&format!("/play/p{handled}/seg1.ts"));
        assert!(!m.handle_wedged_relay(&local, 1));
    }
}

#[test]
fn audio_device_count_is_tracked_once_per_change() {
    let r = Rig::new();
    let mut m = r.monitor();
    let none = "2026.09.29 13:23:29 Debug      -  uSpeak: SetInputDevice 0 (0 total, index out of range, setting to default device) 'No device available'\n";
    r.feed(&mut m, none);
    assert!(m.audio_devices_missing());
    r.feed(&mut m, none);
    assert!(m.audio_devices_missing());
    r.feed(
        &mut m,
        "2026.09.29 13:26:28 Debug      -  uSpeak: SetInputDevice 0 (5 total) 'Microphone (2- fifine Microphone)'\n",
    );
    assert!(!m.audio_devices_missing());
    assert_eq!(r.probes.load(Ordering::SeqCst), 1);
    let text = r.out.text();
    assert_eq!(text.matches("VRChat found no audio devices").count(), 1);
    assert!(text.contains("VRChat's audio devices are back"));
}

#[test]
fn load_failures_are_only_blamed_when_sound_output_works() {
    for (no_devices, state, blamed) in [
        (true, AudioState::InUse, false),
        (true, AudioState::NoDevice, false),
        (true, AudioState::Ok, true),
        (true, AudioState::Unknown, true),
        (false, AudioState::InUse, true),
    ] {
        let r = Rig::new().with_cache().with_gate();
        let mut m = r.monitor_with(r.deps(AudioCheck::new(state, Some("Speakers (3- fifine Microphone)"))));
        if no_devices {
            r.feed(
                &mut m,
                "uSpeak: SetInputDevice 0 (0 total, index out of range) 'No device available'\n",
            );
        }
        for _ in 0..health::OPEN_THRESHOLD {
            r.store(PLAYBACK);
            r.feed(
                &mut m,
                &format!("[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Error: Loading failed\n"),
            );
        }
        let label = format!("{no_devices} {state:?}");
        assert_eq!(r.gate_open(), blamed, "{label}");
        assert_eq!(
            r.hint.as_ref().is_some_and(|h| h.should_prefer_og(SOURCE)),
            blamed,
            "{label}"
        );
        assert_eq!(r.cached(PLAYBACK), !blamed, "{label}");
        let probes = if no_devices { 1 + health::OPEN_THRESHOLD } else { 0 };
        assert_eq!(r.probes.load(Ordering::SeqCst), probes, "{label}");
        let text = r.out.text();
        if !blamed {
            assert!(text.contains("VRChat has no sound output: "), "{text}");
            assert!(
                text.contains("video failed to load because VRChat has no sound output: "),
                "{text}"
            );
            assert!(text.contains("(not a resolver fault)"));
            assert!(r.sink.all().is_empty());
            assert!(r.file_log().contains(&format!(
                "load_failure_no_sound_output ms=0 state={}",
                state.name()
            )));
        }
    }
}

#[test]
fn audio_helpers_match_the_csharp_tables() {
    assert_eq!(
        audio::no_sound_reason(&AudioCheck::new(
            AudioState::InUse,
            Some("Speakers (3- fifine Microphone)")
        )),
        "Windows' default playback device 'Speakers (3- fifine Microphone)' is held by another app in exclusive mode (Voicemeeter, ASIO or KS outputs can do this)"
    );
    assert!(
        audio::no_sound_reason(&AudioCheck::new(AudioState::InUse, None))
            .starts_with("Windows' default playback device is held")
    );
    assert_eq!(
        audio::no_sound_reason(&AudioCheck::new(AudioState::NoDevice, None)),
        "Windows has no default playback device"
    );
    for (code, state) in [
        (0, AudioState::Ok),
        (4, AudioState::InUse),
        (2, AudioState::NoDevice),
        (6, AudioState::NoDevice),
        (1, AudioState::Unknown),
    ] {
        assert_eq!(audio::classify(code), state, "{code}");
    }
    assert_eq!(
        audio::from_probe(DefaultOutput::NoDevices).state,
        AudioState::NoDevice
    );
    assert_eq!(
        audio::from_probe(DefaultOutput::NoPreferred).state,
        AudioState::NoDevice
    );
    assert_eq!(
        audio::from_probe(DefaultOutput::Unknown).state,
        AudioState::Unknown
    );
    assert_eq!(
        audio::from_probe(DefaultOutput::Opened {
            result: 4,
            device: Some("Out".to_owned())
        }),
        AudioCheck::new(AudioState::InUse, Some("Out"))
    );
    assert!(AudioCheck::new(AudioState::InUse, None).blocks_playback());
    assert!(!AudioCheck::new(AudioState::Unknown, None).blocks_playback());
}

#[test]
fn the_real_audio_probe_returns() {
    let check = audio::check_default();
    assert!(check.device.as_deref().is_none_or(|d| !d.is_empty()));
}

#[test]
fn delivered_heights_are_capped() {
    let r = Rig::new();
    let mut m = r.monitor();
    for i in 0..70 {
        r.clock.advance(ms(10));
        r.feed(
            &mut m,
            &format!("[AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/v.m3u8?q={i}\nSwitched to 640x360\n"),
        );
    }
    let now = r.now();
    let kept = (0..70)
        .filter(|i| {
            m.observed_delivered_height(
                &format!("https://us1.vrcresolver.com/api/proxy/v.m3u8?q={i}"),
                now,
            )
            .is_some()
        })
        .count();
    assert_eq!(kept, vrclog::MAX_DELIVERED_HEIGHT_ENTRIES);
    assert!(
        m.observed_delivered_height("https://us1.vrcresolver.com/api/proxy/v.m3u8?q=0", now)
            .is_none()
    );
    assert!(
        m.observed_delivered_height("https://us1.vrcresolver.com/api/proxy/v.m3u8?q=69", now)
            .is_some()
    );
}

#[tokio::test]
async fn the_driver_tails_a_real_log_and_fires_the_stall_timer() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let log = logs.write("output_log_a.txt", "boot\n");
    let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
    deps.silent_stall_window = ms(300);
    let monitor = r.monitor_with(deps);
    let cancel = CancellationToken::new();
    let task = monitor.spawn(vrcr_watchdog::clock::system(), cancel.clone());
    tokio::time::sleep(ms(200)).await;
    logs.append(&log, &format!("{}\n", opening(PLAYBACK)));
    let deadline = tokio::time::Instant::now() + secs(5);
    while r.sink.all().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(ms(25)).await;
    }
    let all = r.sink.all();
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].0.as_str(), all[0].1), (PLAYBACK, feedback::SILENT_STALL));
    assert!((300..1500).contains(&all[0].2), "{}", all[0].2);
    cancel.cancel();
    assert!(tokio::time::timeout(secs(2), task).await.is_ok());
}

#[test]
fn uptime_and_bytes_formats() {
    for (s, expected) in [
        (0, "0m"),
        (45, "0m"),
        (60, "1m"),
        (59 * 60, "59m"),
        (3600, "1h0m"),
        (2 * 3600 + 13 * 60, "2h13m"),
        (24 * 3600, "1d0h"),
        (50 * 3600 + 30 * 60, "2d2h"),
    ] {
        assert_eq!(heartbeat::format_uptime(secs(s)), expected, "{s}");
    }
    for (b, expected) in [
        (0, "0 B"),
        (1023, "1023 B"),
        (1024, "1.0 KB"),
        (1024 * 1024, "1.0 MB"),
        (1024 * 1024 * 1024, "1.00 GB"),
        (1024_u64.pow(4), "1.00 TB"),
        (1_288_490_188, "1.20 GB"),
    ] {
        assert_eq!(format_bytes(b), expected, "{b}");
    }
    assert_eq!(format_bytes_per_second(2048), "2.0 KB/s");
}

#[test]
fn heartbeat_line_lists_only_what_happened() {
    let quiet = Activity::default();
    assert_eq!(
        heartbeat::line(secs(120), false, &quiet, 0),
        "up=2m mesh=disconnected"
    );
    let busy = Activity {
        resolves_total: 12,
        resolves_via_lh_yt: 3,
        resolves_cache_hits: 4,
        bytes_estimate_total: 1024 * 1024 * 5,
        reconnects: 2,
        ..Activity::default()
    };
    assert_eq!(
        heartbeat::line(secs(3 * 3600 + 5 * 60), true, &busy, 7),
        "up=3h5m mesh=connected resolves=12 (3 local video) video=5.0 MB cache=7(4hits) reconnects=2"
    );
    let hits_only = Activity {
        resolves_cache_hits: 1,
        ..Activity::default()
    };
    assert_eq!(
        heartbeat::line(secs(60), true, &hits_only, 0),
        "up=1m mesh=connected cache=0(1hits)"
    );
}

#[test]
fn heartbeat_stays_quiet_while_the_log_is_busy() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let clock = ManualClock::at(SystemTime::now());
    let out = Capture::default();
    let log = Log::to_file(&dir.path().join("w.log"));
    let stats = Arc::new(Stats::new(clock.get() - secs(90 * 60)));
    stats.record_resolve(true);
    let hb = Heartbeat {
        stats,
        mesh_connected: Box::new(|| true),
        cache_size: Box::new(|| 2),
        now: clock.now(),
        ux: Ux::new(Box::new(out.clone()), Style::plain(), String::new),
        log: log.clone(),
    };
    log.line("recent activity");
    assert_eq!(hb.tick(), None);
    clock.advance(heartbeat::QUIET_WINDOW + secs(5));
    assert_eq!(
        hb.tick().as_deref(),
        Some("up=1h35m mesh=connected resolves=1 (1 local video) cache=2(0hits)")
    );
    assert!(out.text().contains("[heartbeat]"));

    let silent = Heartbeat {
        log: Log::disabled(),
        ..hb
    };
    assert!(silent.tick().is_some());
}
