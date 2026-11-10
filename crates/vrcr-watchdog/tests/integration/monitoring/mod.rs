use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use crate::common::ManualClock;
use crate::common::mesh::Capture;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{ResolveStatus, Style, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::iso8601_utc;
use vrcr_shared::trust_gateway;
use vrcr_shared::win::{DefaultOutput, MemoryStatus, ProcessUsage};
use vrcr_shared::wire::{ResolveResponse, action, feedback};
use vrcr_watchdog::display::{format_bytes, format_bytes_per_second};
use vrcr_watchdog::health::{self, HealthGate};
use vrcr_watchdog::ipc::Feedback;
use vrcr_watchdog::monitoring::audio::{self, AudioCheck, AudioState};
use vrcr_watchdog::monitoring::heartbeat::{self, Heartbeat};
use vrcr_watchdog::monitoring::perf::{self, Phases, SystemInfo};
use vrcr_watchdog::monitoring::vrclog::{
    self, MonitorDeps, PlaybackSink, RelayWedge, VrcLogMonitor, initial_read_offset_for_new_file,
    parse_observed_resolution, should_rescan_directory,
};
use vrcr_watchdog::og_hint::{self, OgFallbackHint};
use vrcr_watchdog::relay::RelayActivity;
use vrcr_watchdog::resolve_cache::{Key, ResolveCache};
use vrcr_watchdog::stats::{
    Activity, PERF_SAMPLE_CAP, PerfSnapshot, RelayKind, Series, Spread, Stats,
};

const T0: u64 = crate::common::T_2026_09_29;
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
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(feedback);
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
            Arc::<Sink>::clone(&self.sink),
            Arc::new(RwLock::new(FirstPartyPolicy::default())),
            Ux::new(Box::new(self.out.clone()), Style::plain(), String::new),
            self.log.clone(),
        );
        d.cache = self.cache.clone();
        d.og_hint = self.hint.clone();
        d.health = self.gate.clone();
        d.silent_stall_window = secs(300);
        let probes = std::sync::Arc::clone(&self.probes);
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

mod audio_cases;

mod contracts_cases;

mod heartbeat_cases;

mod local_cases;

mod logs_cases;

mod performance_cases;

mod playback_cases;
