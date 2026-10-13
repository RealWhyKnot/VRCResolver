use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, SystemTime};

use regex_lite::Regex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::redact_url;
use vrcr_shared::paths::AppPaths;
use vrcr_shared::trust_gateway::extract_target;
use vrcr_shared::wire::feedback;

use crate::clock::Now;
use crate::health::{self, HealthGate, Transition};
use crate::ipc::{Feedback, Mesh};
use crate::monitoring::audio::{self, AudioCheck, NO_SOUND_FIX};
use crate::og_hint::OgFallbackHint;
use crate::relay::RelayActivity;
use crate::resolve_cache::ResolveCache;

pub const AVPRO_OPEN_FAIL_WINDOW: Duration = Duration::from_secs(10);
pub const DELIVERED_HEIGHT_TTL: Duration = Duration::from_secs(10 * 60);
pub const PLAYING_FEEDBACK_INTERVAL: Duration = Duration::from_secs(15);
pub const DEFAULT_SILENT_STALL_WINDOW: Duration = Duration::from_secs(12);
pub const MAX_DELIVERED_HEIGHT_ENTRIES: usize = 64;
pub const MIN_DELIVERED_HEIGHT: i32 = 16;
pub const MAX_DELIVERED_HEIGHT: i32 = 4320;
pub const DIRECTORY_RESCAN_INTERVAL_MS: i64 = 5000;
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
pub const MISSING_DIR_INTERVAL: Duration = Duration::from_secs(5);

static AVPRO_OPENING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\[AVProVideo\] Opening\s+(\S+)").unwrap_or_else(|e| panic!("{e}")));
static SWITCHED_RESOLUTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bSwitched\s+to(?:\s+Resolution)?\s+([0-9]{2,5})x([0-9]{2,5})\b")
        .unwrap_or_else(|e| panic!("{e}"))
});
static AVPRO_STATE_RESOLUTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bfwidth=([0-9]{2,5})\b.*\bfheight=([0-9]{2,5})\b").unwrap_or_else(|e| panic!("{e}"))
});
static AUDIO_DEVICE_COUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"uSpeak: SetInputDevice [0-9]+ \(([0-9]+) total").unwrap_or_else(|e| panic!("{e}"))
});

pub trait PlaybackSink: Send + Sync {
    fn send_playback_feedback(&self, feedback: Feedback);
}

impl<T: Mesh> PlaybackSink for T {
    fn send_playback_feedback(&self, feedback: Feedback) {
        Mesh::send_playback_feedback(self, feedback);
    }
}

pub struct RelayWedge {
    pub activity: Arc<RelayActivity>,
    pub on_wedged: Box<dyn Fn(u16) -> bool + Send + Sync>,
}

pub struct MonitorDeps {
    pub mesh: Arc<dyn PlaybackSink>,
    pub policy: Arc<RwLock<FirstPartyPolicy>>,
    pub cache: Option<Arc<ResolveCache>>,
    pub og_hint: Option<Arc<OgFallbackHint>>,
    pub health: Option<Arc<HealthGate>>,
    pub relay_wedge: Option<RelayWedge>,
    pub on_new_session: Option<Box<dyn Fn() + Send + Sync>>,
    pub audio: Box<dyn Fn() -> AudioCheck + Send + Sync>,
    pub silent_stall_window: Duration,
    pub ux: Ux,
    pub log: Log,
}

impl MonitorDeps {
    pub fn new(
        mesh: Arc<dyn PlaybackSink>,
        policy: Arc<RwLock<FirstPartyPolicy>>,
        ux: Ux,
        log: Log,
    ) -> MonitorDeps {
        MonitorDeps {
            mesh,
            policy,
            cache: None,
            og_hint: None,
            health: None,
            relay_wedge: None,
            on_new_session: None,
            audio: Box::new(audio::check_default),
            silent_stall_window: DEFAULT_SILENT_STALL_WINDOW,
            ux,
            log,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Recovery {
    pub evicted: usize,
    pub og_hint_armed: bool,
}

impl Recovery {
    fn suffix(self) -> String {
        let mut s = String::new();
        if self.evicted > 0 {
            s.push_str(&format!(" evicted={}", self.evicted));
        }
        if self.og_hint_armed {
            s.push_str(" og_hint=armed");
        }
        s
    }
}

#[derive(Default)]
struct Tail {
    offset: u64,
    partial: Vec<u8>,
}

struct Stall {
    url: String,
    at: SystemTime,
}

struct Playing {
    url: String,
    opened_at: SystemTime,
    next: SystemTime,
}

pub fn default_vrc_dir() -> PathBuf {
    AppPaths::from_env().local_low().join("VRChat").join("VRChat")
}

pub fn should_rescan_directory(has_current_file: bool, ms_since_last_scan: i64) -> bool {
    !has_current_file || !(0..DIRECTORY_RESCAN_INTERVAL_MS).contains(&ms_since_last_scan)
}

pub fn initial_read_offset_for_new_file(file_length: u64, first_file: bool) -> u64 {
    if first_file { file_length } else { 0 }
}

fn parse_resolution(w: &str, h: &str) -> Option<(i32, i32)> {
    let (w, h) = (w.parse::<i32>().ok()?, h.parse::<i32>().ok()?);
    ((16..=16_384).contains(&w) && (MIN_DELIVERED_HEIGHT..=MAX_DELIVERED_HEIGHT).contains(&h))
        .then_some((w, h))
}

pub fn parse_observed_resolution(line: &str) -> Option<(i32, i32)> {
    if line.trim().is_empty() {
        return None;
    }
    if let Some(c) = SWITCHED_RESOLUTION.captures(line)
        && let Some(r) = parse_resolution(&c[1], &c[2])
    {
        return Some(r);
    }
    let c = AVPRO_STATE_RESOLUTION.captures(line)?;
    parse_resolution(&c[1], &c[2])
}

fn elapsed_ms(start: SystemTime, now: SystemTime) -> i32 {
    let ms = now.duration_since(start).unwrap_or_default().as_millis();
    i32::try_from(ms).unwrap_or(i32::MAX)
}

fn is_log_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("output_log") && lower.ends_with(".txt")
}

fn find_latest_log_file(dir: &Path) -> Option<(PathBuf, u64)> {
    let mut best: Option<(SystemTime, PathBuf, u64)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        if !entry.file_name().to_str().is_some_and(is_log_file) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(m, _, _)| modified > *m) {
            best = Some((modified, entry.path(), meta.len()));
        }
    }
    best.map(|(_, p, len)| (p, len))
}

pub struct VrcLogMonitor {
    deps: MonitorDeps,
    vrc_dir: PathBuf,
    current: Option<PathBuf>,
    tails: HashMap<PathBuf, Tail>,
    partial: Vec<u8>,
    last_scan: Option<SystemTime>,
    last_opening: Option<(String, SystemTime)>,
    active: Option<(String, SystemTime)>,
    stall: Option<Stall>,
    playing: Option<Playing>,
    delivered: HashMap<String, (i32, SystemTime)>,
    audio_missing: bool,
}

impl VrcLogMonitor {
    pub fn new(deps: MonitorDeps, vrc_dir: PathBuf) -> VrcLogMonitor {
        VrcLogMonitor {
            deps,
            vrc_dir,
            current: None,
            tails: HashMap::new(),
            partial: Vec::new(),
            last_scan: None,
            last_opening: None,
            active: None,
            stall: None,
            playing: None,
            delivered: HashMap::new(),
            audio_missing: false,
        }
    }

    pub fn vrc_dir(&self) -> &Path {
        &self.vrc_dir
    }

    pub fn last_opened_url(&self) -> Option<&str> {
        self.last_opening.as_ref().map(|(u, _)| u.as_str())
    }

    pub fn audio_devices_missing(&self) -> bool {
        self.audio_missing
    }

    pub fn stall_armed(&self) -> bool {
        self.stall.is_some()
    }

    pub fn playing_feedback_url(&self) -> Option<&str> {
        self.playing.as_ref().map(|p| p.url.as_str())
    }

    pub fn observed_delivered_height(&mut self, url: &str, now: SystemTime) -> Option<i32> {
        let canonical = self.canonical(url);
        self.delivered_height(&canonical, now)
    }

    pub fn canonical(&self, url: &str) -> String {
        extract_target(url)
            .filter(|t| self.policy().is_playback_proxy_url(t))
            .unwrap_or_else(|| url.to_owned())
    }

    fn policy(&self) -> std::sync::RwLockReadGuard<'_, FirstPartyPolicy> {
        self.deps.policy.read().unwrap_or_else(|e| e.into_inner())
    }

    fn is_attributed(&self, url: &str) -> bool {
        !url.is_empty()
            && (self.policy().is_playback_proxy_url(url)
                || self
                    .deps
                    .cache
                    .as_ref()
                    .is_some_and(|c| c.source_url_for_resolved(url).is_some()))
    }

    fn confirm_playback(&self, url: &str) {
        let Some(gate) = &self.deps.health else { return };
        if !self.is_attributed(url) {
            return;
        }
        if gate.record_playback_confirmed() == Transition::Closed {
            self.deps.ux.success(
                Component::VrcLog,
                "resolver restored -- mesh resolving re-enabled",
            );
        }
    }

    pub fn poll_once(&mut self, now: SystemTime) {
        let since = self.last_scan.map_or(i64::MAX, |t| {
            now.duration_since(t)
                .map_or(-1, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        });
        let current_meta = || {
            self.current.as_ref().and_then(|p| {
                fs::metadata(p)
                    .ok()
                    .filter(|m| m.is_file())
                    .map(|m| (p.clone(), m.len()))
            })
        };
        let latest = if should_rescan_directory(self.current.is_some(), since) {
            self.last_scan = Some(now);
            find_latest_log_file(&self.vrc_dir)
        } else if let Some(found) = current_meta() {
            Some(found)
        } else {
            self.last_scan = Some(now);
            find_latest_log_file(&self.vrc_dir)
        };
        let Some((path, len)) = latest else { return };
        if self.current.as_ref() != Some(&path) {
            self.switch_to(path.clone(), len);
        }
        let offset = self.tails.get(&path).map_or(0, |t| t.offset);
        if len > offset {
            if let Some(bytes) = read_from(&path, offset) {
                if let Some(t) = self.tails.get_mut(&path) {
                    t.offset = offset + bytes.len() as u64;
                }
                let content = if offset == 0 {
                    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)
                } else {
                    &bytes[..]
                };
                self.process_bytes(content, now);
            }
        } else if len < offset {
            if let Some(t) = self.tails.get_mut(&path) {
                t.offset = 0;
            }
            self.partial.clear();
        }
    }

    fn switch_to(&mut self, path: PathBuf, len: u64) {
        let first_file = self.current.is_none();
        if let Some(old) = self.current.take()
            && let Some(t) = self.tails.get_mut(&old)
        {
            t.partial = std::mem::take(&mut self.partial);
        }
        let seen_before = self.tails.contains_key(&path);
        let tail = self.tails.entry(path.clone()).or_insert_with(|| Tail {
            offset: initial_read_offset_for_new_file(len, first_file),
            partial: Vec::new(),
        });
        self.partial = std::mem::take(&mut tail.partial);
        self.current = Some(path);
        self.audio_missing = false;
        if !seen_before
            && !first_file
            && let Some(cb) = &self.deps.on_new_session
        {
            cb();
        }
    }

    pub fn process_text(&mut self, content: &str, now: SystemTime) {
        self.process_bytes(content.as_bytes(), now);
    }

    pub fn process_bytes(&mut self, content: &[u8], now: SystemTime) {
        let mut pending = std::mem::take(&mut self.partial);
        pending.extend_from_slice(content);
        let Some(last_nl) = pending.iter().rposition(|&b| b == b'\n') else {
            self.partial = pending;
            return;
        };
        self.partial = pending[last_nl + 1..].to_vec();
        for raw in pending[..last_nl].split(|&b| b == b'\n') {
            let text = String::from_utf8_lossy(raw);
            let line = text.trim();
            if !line.is_empty() {
                self.process_line(line, now);
            }
        }
    }

    fn process_line(&mut self, line: &str, now: SystemTime) {
        if line.contains("[Behaviour] OnLeftRoom") || line.contains("[Behaviour] Entering Room:") {
            self.stop_watching("world_change");
            return;
        }
        if let Some(c) = AUDIO_DEVICE_COUNT.captures(line) {
            self.on_audio_count(&c[1] == "0");
            return;
        }
        if let Some(c) = AVPRO_OPENING.captures(line) {
            self.on_opening(c[1].to_owned(), now);
            return;
        }
        if let Some((_, height)) = parse_observed_resolution(line)
            && let Some((url, at)) = self.active.clone()
        {
            self.remember_delivered_height(&url, height, now);
            self.confirm_playback(&url);
            self.start_playing(&url, at, now);
            self.send(&url, feedback::PLAYING, elapsed_ms(at, now), Some(height));
            return;
        }
        if line.contains("[AVProVideo] Error: Loading failed") {
            self.on_load_failed(now);
            return;
        }
        let accepted = line.contains("[AVProVideo] Using playback path:");
        if accepted || line.contains("Now Playing:") || line.contains("Load Url:") {
            if accepted && let Some((url, _)) = self.active.clone() {
                self.confirm_playback(&url);
            }
            self.stall = None;
        }
    }

    fn on_audio_count(&mut self, missing: bool) {
        let ux = &self.deps.ux;
        if missing && !self.audio_missing {
            let audio = (self.deps.audio)();
            if audio.blocks_playback() {
                ux.error(
                    Component::VrcLog,
                    &format!(
                        "VRChat has no sound output: {}. Videos will fail to load until you {NO_SOUND_FIX}",
                        audio::no_sound_reason(&audio)
                    ),
                );
            } else {
                ux.warn(
                    Component::VrcLog,
                    "VRChat found no audio devices; AVPro videos can fail to load until Windows audio recovers",
                );
            }
        } else if !missing && self.audio_missing {
            ux.write(Component::VrcLog, "VRChat's audio devices are back");
        }
        self.audio_missing = missing;
    }

    fn on_opening(&mut self, opened: String, now: SystemTime) {
        let canonical = self.canonical(&opened);
        self.last_opening = Some((opened.clone(), now));
        self.active = Some((canonical.clone(), now));
        self.playing = None;
        self.stall = Some(Stall { url: opened, at: now });
        if self.delivered_height(&canonical, now).is_some() {
            self.start_playing(&canonical, now, now);
        }
    }

    fn on_load_failed(&mut self, now: SystemTime) {
        let recent = self.last_opening.take_if(|(_, at)| {
            now.duration_since(*at)
                .map_or(true, |d| d <= AVPRO_OPEN_FAIL_WINDOW)
        });
        if let Some((observed, at)) = recent {
            self.report_load_failure(&observed, elapsed_ms(at, now), now);
        }
        self.clear_playback();
    }

    fn report_load_failure(&mut self, observed: &str, ms: i32, now: SystemTime) {
        let failed = self.canonical(observed);
        if self.audio_missing {
            let audio = (self.deps.audio)();
            if audio.blocks_playback() {
                self.deps.ux.error(
                    Component::VrcLog,
                    &format!(
                        "video failed to load because VRChat has no sound output: {}. To fix it, {NO_SOUND_FIX} (not a resolver fault)",
                        audio::no_sound_reason(&audio)
                    ),
                );
                self.deps.log.line(&format!(
                    "[vrclog] load_failure_no_sound_output ms={ms} state={} url={}",
                    audio.state.name(),
                    redact_url(&failed)
                ));
                return;
            }
        }
        let height = self.delivered_height(&failed, now);
        self.send(&failed, feedback::LOAD_FAILURE, ms, height);
        let recovery = self.mark_playback_failure(&failed);
        self.deps.ux.warn(
            Component::VrcLog,
            &format!(
                "load_failure ms={ms} url={}{}",
                redact_url(&failed),
                recovery.suffix()
            ),
        );
    }

    fn clear_playback(&mut self) {
        self.stall = None;
        self.playing = None;
        self.active = None;
    }

    pub fn next_deadline(&self) -> Option<SystemTime> {
        let stall = self.stall.as_ref().map(|s| s.at + self.deps.silent_stall_window);
        let playing = self.playing.as_ref().map(|p| p.next);
        match (stall, playing) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn fire_due(&mut self, now: SystemTime) {
        if self
            .stall
            .as_ref()
            .is_some_and(|s| now >= s.at + self.deps.silent_stall_window)
            && let Some(stall) = self.stall.take()
        {
            self.on_silent_stall(&stall.url, elapsed_ms(stall.at, now), now);
        }
        if self.playing.as_ref().is_some_and(|p| now >= p.next)
            && let Some(p) = self.playing.take()
            && let Some(height) = self.delivered_height(&p.url, now)
        {
            self.send(
                &p.url,
                feedback::PLAYING,
                elapsed_ms(p.opened_at, now),
                Some(height),
            );
            self.playing = Some(Playing {
                next: now + PLAYING_FEEDBACK_INTERVAL,
                ..p
            });
        }
    }

    fn on_silent_stall(&mut self, opened: &str, ms: i32, now: SystemTime) {
        if self.handle_wedged_relay(opened, ms) {
            self.playing = None;
            self.active = None;
            return;
        }
        let reported = self.canonical(opened);
        let height = self.delivered_height(&reported, now);
        self.send(&reported, feedback::SILENT_STALL, ms, height);
        self.playing = None;
        self.active = None;
        let recovery = self.mark_playback_failure(&reported);
        self.deps.ux.warn(
            Component::VrcLog,
            &format!(
                "silent_stall ms={ms} url={}{}",
                redact_url(&reported),
                recovery.suffix()
            ),
        );
    }

    pub fn handle_wedged_relay(&self, opened: &str, ms: i32) -> bool {
        let Some(wedge) = &self.deps.relay_wedge else {
            return false;
        };
        let Some(port) = wedge.activity.wedged_port(opened) else {
            return false;
        };
        let handled = (wedge.on_wedged)(port);
        self.deps.ux.warn(
            Component::VrcLog,
            &format!(
                "relay_pool_wedged ms={ms} port={port}: VRChat never reached the local relay{}",
                if handled {
                    ""
                } else {
                    "; counting as a failed play"
                }
            ),
        );
        handled
    }

    pub fn mark_playback_failure(&self, reported: &str) -> Recovery {
        let attributed = self.is_attributed(reported);
        let mut og_hint_armed = false;
        if let (Some(hint), Some(cache)) = (&self.deps.og_hint, &self.deps.cache)
            && let Some(source) = cache.source_url_for_resolved(reported)
        {
            hint.record_load_failure(&source);
            og_hint_armed = true;
        }
        let evicted = self.deps.cache.as_ref().map_or(0, |c| c.evict_by_url(reported));
        if attributed
            && let Some(gate) = &self.deps.health
            && gate.record_playback_failure() == Transition::Opened
        {
            self.deps.ux.warn(
                Component::VrcLog,
                &format!(
                    "resolver paused -- {} playbacks failed in a row; VRChat's own resolver takes over while things recover",
                    health::OPEN_THRESHOLD
                ),
            );
        }
        Recovery {
            evicted,
            og_hint_armed,
        }
    }

    fn send(&self, url: &str, kind: &'static str, ms: i32, height: Option<i32>) {
        self.deps.mesh.send_playback_feedback(Feedback {
            url: url.to_owned(),
            kind,
            ms_since_open: ms,
            delivered_height: height,
            detail: None,
            correlation_id: None,
        });
    }

    fn start_playing(&mut self, url: &str, opened_at: SystemTime, now: SystemTime) {
        if url.trim().is_empty() || self.playing.as_ref().is_some_and(|p| p.url == url) {
            return;
        }
        self.playing = Some(Playing {
            url: url.to_owned(),
            opened_at,
            next: now + PLAYING_FEEDBACK_INTERVAL,
        });
    }

    fn remember_delivered_height(&mut self, url: &str, height: i32, now: SystemTime) {
        if url.trim().is_empty() || height <= 0 {
            return;
        }
        self.delivered.insert(url.to_owned(), (height, now));
        self.delivered.retain(|_, (_, at)| !expired(*at, now));
        while self.delivered.len() > MAX_DELIVERED_HEIGHT_ENTRIES {
            let Some(oldest) = self
                .delivered
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.delivered.remove(&oldest);
        }
    }

    fn delivered_height(&mut self, url: &str, now: SystemTime) -> Option<i32> {
        if url.trim().is_empty() {
            return None;
        }
        let (height, at) = *self.delivered.get(url)?;
        if expired(at, now) {
            self.delivered.remove(url);
            return None;
        }
        Some(height)
    }

    fn stop_watching(&mut self, reason: &str) {
        let watched = self
            .active
            .take()
            .map(|(u, _)| u)
            .or_else(|| self.last_opening.as_ref().map(|(u, _)| u.clone()));
        self.stall = None;
        self.playing = None;
        self.last_opening = None;
        if let Some(url) = watched {
            self.deps.log.line(&format!(
                "[vrclog] stopped watching {} reason={reason}",
                redact_url(&url)
            ));
        }
    }

    pub fn spawn(mut self, now: Now, cancel: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let dir_present = self.vrc_dir.is_dir();
                if dir_present {
                    self.poll_once(now());
                }
                let t = now();
                self.fire_due(t);
                let mut wait = if dir_present {
                    POLL_INTERVAL
                } else {
                    MISSING_DIR_INTERVAL
                };
                if let Some(deadline) = self.next_deadline() {
                    wait = wait.min(deadline.duration_since(t).unwrap_or_default());
                }
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(wait) => {}
                }
            }
        })
    }
}

fn expired(at: SystemTime, now: SystemTime) -> bool {
    now.duration_since(at).is_ok_and(|d| d > DELIVERED_HEIGHT_TTL)
}

fn read_from(path: &Path, offset: u64) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}
