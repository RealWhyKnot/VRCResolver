mod requests;
mod video_failures;

pub mod resolve;
pub mod startup;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use url::Url;
use vrcr_shared::console::{Component, PlaybackError, Ux};
use vrcr_shared::wire::{ResolveRequest, ResolveResponse};

use crate::relay::hitch::{TimingSample, relay_kind};
use crate::stats::RelayKind;

const CAPACITY: usize = 256;
const IDLE: Duration = Duration::from_secs(300);
const REPEAT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct PlaybackLog {
    plays: Mutex<HashMap<String, Play>>,
    video_failures: Mutex<HashMap<String, SystemTime>>,
}

struct Play {
    id: String,
    aliases: HashSet<String>,
    host: String,
    resolver_origin: Option<String>,
    source_url: String,
    resolved_url: Option<String>,
    reported_suppressed: u64,
    reported_recovery: u64,
    last_retry_report: Option<SystemTime>,
    started: SystemTime,
    seen: SystemTime,
    manifests: u64,
    segments: u64,
    other: u64,
    active: HashSet<String>,
    end_reason: Option<String>,
    retries: u64,
    bytes: u64,
    manifest_max: i64,
    segment_max: i64,
    reasons: BTreeMap<String, u64>,
    statuses: BTreeMap<String, u64>,
    failures: BTreeMap<String, Repeat>,
    console_errors: HashSet<(String, PlaybackError)>,
    playlists: HashSet<String>,
    context: FailureContext,
}

struct Repeat {
    count: u64,
    reported: u64,
    at: SystemTime,
    line: String,
    target: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FailureContext {
    pub playback_id: String,
    pub request_id: String,
    pub stream_id: String,
    pub source: String,
    pub upstream_status: u16,
    pub suppressed_requests: u64,
    pub recovery_attempts: u64,
    pub recovery_result: String,
    pub retry_reason: String,
}

#[derive(Clone, Copy)]
pub struct ResolveDelivery {
    pub cached: bool,
    pub retried: bool,
    pub bytes: usize,
}

pub fn token(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
    {
        value.to_owned()
    } else {
        "unknown".to_owned()
    }
}

pub fn player_error(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let name = if lower.contains("loading failed") || lower.contains("loading_failed") {
        "loading_failed"
    } else if lower.contains("silent_stall") {
        "silent_stall"
    } else if lower.contains("decode") {
        "decode_failed"
    } else {
        "player_error"
    };
    for marker in ["errorcode:", "errorcode=", "error code:", "code="] {
        if let Some((_, tail)) = lower.split_once(marker) {
            let code: String = tail
                .trim_start()
                .chars()
                .take_while(|character| {
                    character.is_ascii_hexdigit() || *character == 'x' || *character == '-'
                })
                .take(12)
                .collect();
            if !code.is_empty() {
                return format!("{name},code={code}");
            }
        }
    }
    name.to_owned()
}

pub fn error_kind(value: &str) -> &'static str {
    let lower = value.to_ascii_lowercase();
    for (word, reason) in [
        ("http 503", "http_503"),
        ("http 502", "http_502"),
        ("http 404", "http_404"),
        ("handshake", "handshake_error"),
        ("timed out", "timeout"),
        ("timeout", "timeout"),
        ("certificate", "tls_error"),
        ("reset", "connection_reset"),
        ("refused", "connection_refused"),
        ("closed", "connection_closed"),
        ("dns", "dns_error"),
        ("rate", "rate_limited"),
        ("format", "format_error"),
        ("decode", "decode_error"),
        ("ffmpeg", "transcode_error"),
        ("parse", "parse_error"),
    ] {
        if lower.contains(word) {
            return reason;
        }
    }
    "unspecified"
}

pub fn host(value: &str) -> String {
    Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .filter(|host| host.parse::<std::net::IpAddr>().is_err())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn url_key(value: &str) -> String {
    let normalized = Url::parse(value).map_or_else(|_| value.to_owned(), |url| url.to_string());
    crate::relay::manifest::target_namespace(&normalized)
}

pub fn stream_id(value: &str) -> String {
    let Some(url) = Url::parse(value).ok() else {
        return "unknown".to_owned();
    };
    let parts: Vec<_> = url.path().split('/').collect();
    parts
        .windows(2)
        .find(|pair| pair[0] == "lazy-hls")
        .map_or_else(|| "unknown".to_owned(), |pair| token(pair[1]))
}

fn elapsed(start: SystemTime, now: SystemTime) -> u128 {
    now.duration_since(start).unwrap_or_default().as_millis()
}

pub(crate) fn counts(values: &BTreeMap<String, u64>) -> String {
    if values.is_empty() {
        return "none".to_owned();
    }
    values
        .iter()
        .map(|(key, count)| format!("{key}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn count(values: &mut BTreeMap<String, u64>, key: String) {
    if values.len() < 64 || values.contains_key(&key) {
        *values.entry(key).or_default() += 1;
    } else {
        *values.entry("other".to_owned()).or_default() += 1;
    }
}

fn emit(ux: &Ux, lines: Vec<String>) {
    for line in lines {
        ux.log_write(Component::Relay, &line);
    }
}

impl Play {
    fn new(id: String, target: &str, now: SystemTime) -> Self {
        Self {
            context: FailureContext {
                playback_id: id.clone(),
                request_id: id.clone(),
                source: "unknown".to_owned(),
                ..Default::default()
            },
            id,
            aliases: HashSet::from([url_key(target)]),
            host: host(target),
            resolver_origin: None,
            source_url: target.to_owned(),
            resolved_url: None,
            reported_suppressed: 0,
            reported_recovery: 0,
            last_retry_report: None,
            started: now,
            seen: now,
            manifests: 0,
            segments: 0,
            other: 0,
            active: HashSet::new(),
            end_reason: None,
            retries: 0,
            bytes: 0,
            manifest_max: 0,
            segment_max: 0,
            reasons: BTreeMap::new(),
            statuses: BTreeMap::new(),
            failures: BTreeMap::new(),
            console_errors: HashSet::new(),
            playlists: HashSet::new(),
        }
    }

    fn alias(&mut self, key: String) {
        if self.aliases.len() < 16 {
            self.aliases.insert(key);
        }
    }

    fn close(self, reason: &str, now: SystemTime) -> Vec<String> {
        let mut lines = self
            .failures
            .values()
            .filter(|repeat| repeat.reported != repeat.count)
            .map(|repeat| {
                format!(
                    "{} count={} suppressed={}",
                    repeat.line, repeat.count, self.context.suppressed_requests
                )
            })
            .collect::<Vec<_>>();
        lines.push(format!(
            "session playback={} host={} duration_ms={} manifests={} segments={} other={} bytes={} slowest_manifest_ms={} slowest_segment_ms={} hitches={} retries={} statuses={} ended={} suppressed={} recovery_attempts={} recovery={}",
            self.id, self.host, elapsed(self.started, now), self.manifests, self.segments, self.other,
            self.bytes, self.manifest_max, self.segment_max, counts(&self.reasons),
            self.retries, counts(&self.statuses), token(reason), self.context.suppressed_requests, self.context.recovery_attempts, if self.context.recovery_result.is_empty() { "none" } else { &self.context.recovery_result },
        ));
        lines
    }
}

impl PlaybackLog {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Play>> {
        self.plays.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn make_room(plays: &mut HashMap<String, Play>, now: SystemTime) -> Vec<String> {
        if plays.len() < CAPACITY {
            return Vec::new();
        }
        let oldest = plays
            .iter()
            .min_by_key(|(_, play)| (!play.active.is_empty(), play.seen))
            .map(|(key, _)| key.clone());
        oldest
            .and_then(|key| plays.remove(&key))
            .map_or_else(Vec::new, |play| play.close("tracking_limit", now))
    }

    pub fn resolving(&self, request: &ResolveRequest, now: SystemTime, ux: &Ux) {
        self.reset_video_failure(&request.url);
        let id = token(&request.id);
        let mut plays = self.lock();
        let mut lines = Self::make_room(&mut plays, now);
        plays.insert(id.clone(), Play::new(id.clone(), &request.url, now));
        drop(plays);
        lines.push(format!(
            "resolve start req={id} host={} player={}",
            host(&request.url),
            request.player.map_or("unknown", |player| player.as_str())
        ));
        emit(ux, lines);
    }

    pub fn resolved(
        &self,
        request: &ResolveRequest,
        response: &ResolveResponse,
        delivery: ResolveDelivery,
        now: SystemTime,
        ux: &Ux,
    ) {
        let cached = delivery.cached;
        let id = token(&request.id);
        let mut plays = self.lock();
        let Some(play) = plays.get_mut(&id) else {
            return;
        };
        if let Some(url) = &response.url {
            play.resolved_url = Some(url.clone());
            play.alias(url_key(url));
            play.context.stream_id = stream_id(url);
            if crate::local::is_play_url(url)
                && let Ok(parsed) = Url::parse(url)
                && let Some(id) = crate::relay::activity::play_id(parsed.path())
            {
                play.alias(format!("play-{}", token(id)));
            }
        }
        let line = format!(
            "resolve done req={id} stream={} host={} action={} engine={} config={} protocol={} container={} video={} audio={} height={} elapsed_ms={} cache={cached} server_req={} reason={} status=unknown bytes={} retry={} sent={}",
            play.context.stream_id,
            play.host,
            token(&response.action),
            token(response.engine.as_deref().unwrap_or_default()),
            token(response.config.as_deref().unwrap_or_default()),
            token(response.protocol.as_deref().unwrap_or_default()),
            token(response.container.as_deref().unwrap_or_default()),
            token(response.video_codec.as_deref().unwrap_or_default()),
            token(response.audio_codec.as_deref().unwrap_or_default()),
            response.resolved_height.unwrap_or(0),
            elapsed(play.started, now),
            token(&response.id),
            token(response.reason.as_deref().unwrap_or("none")),
            delivery.bytes,
            delivery.retried,
            token(&response.action),
        );
        drop(plays);
        ux.log_write(Component::Ipc, &line);
    }

    pub fn resolver_origin(&self, id: &str, host: &str) {
        if let Some(play) = self.lock().get_mut(&token(id)) {
            play.resolver_origin = Url::parse(&format!("https://{host}"))
                .ok()
                .map(|url| url.origin().ascii_serialization());
        }
    }

    pub fn trusts_origin(&self, path: &str, target: &str) -> bool {
        let Some(id) = crate::relay::activity::play_id(path) else {
            return false;
        };
        let Ok(url) = Url::parse(target) else {
            return false;
        };
        let alias = format!("play-{}", token(id));
        self.lock()
            .values()
            .filter(|play| play.aliases.contains(&alias))
            .max_by_key(|play| play.started)
            .is_some_and(|play| {
                play.resolver_origin.as_deref() == Some(url.origin().ascii_serialization().as_str())
            })
    }

    pub fn source_for_path(&self, path: &str) -> Option<String> {
        let alias = format!("play-{}", token(crate::relay::activity::play_id(path)?));
        self.lock()
            .values()
            .filter(|play| play.aliases.contains(&alias))
            .max_by_key(|play| play.started)
            .map(|play| play.source_url.clone())
    }

    pub fn retry_reports(&self, now: SystemTime, force: bool) -> Vec<crate::ipc::Feedback> {
        let mut plays = self.lock();
        plays
            .values_mut()
            .filter_map(|play| {
                let context = &play.context;
                let changed = context.suppressed_requests != play.reported_suppressed
                    || context.recovery_attempts != play.reported_recovery;
                let due = force
                    || play.last_retry_report.is_none_or(|last| {
                        now.duration_since(last)
                            .is_ok_and(|elapsed| elapsed >= REPEAT)
                    })
                    || context.recovery_attempts != play.reported_recovery;
                if !changed || !due {
                    return None;
                }
                play.reported_suppressed = context.suppressed_requests;
                play.reported_recovery = context.recovery_attempts;
                play.last_retry_report = Some(now);
                Some(crate::ipc::Feedback {
                    url: play
                        .resolved_url
                        .as_ref()
                        .unwrap_or(&play.source_url)
                        .clone(),
                    kind: vrcr_shared::wire::feedback::LOAD_FAILURE,
                    ms_since_open: i32::try_from(elapsed(play.started, now)).unwrap_or(i32::MAX),
                    delivered_height: None,
                    detail: Some(context.retry_reason.clone()),
                    correlation_id: Some(play.id.clone()),
                })
            })
            .collect()
    }

    pub fn context(&self, url: &str) -> Option<FailureContext> {
        let key = url_key(url);
        let plays = self.lock();
        let result = plays
            .values()
            .filter(|play| play.aliases.contains(&key))
            .max_by_key(|play| play.started)
            .map(|play| play.context.clone());
        drop(plays);
        result
    }

    pub fn close(&self, url: &str, reason: &str, now: SystemTime, ux: &Ux) {
        let alias = url_key(url);
        let mut plays = self.lock();
        let key = plays
            .iter()
            .filter(|(key, play)| **key == url || play.aliases.contains(&alias))
            .max_by_key(|(_, play)| play.started)
            .map(|(key, _)| key.clone());
        let lines = if let Some(key) = key {
            if plays.get(&key).is_some_and(|play| !play.active.is_empty()) {
                if let Some(play) = plays.get_mut(&key) {
                    play.end_reason = Some(token(reason));
                }
                Vec::new()
            } else {
                plays
                    .remove(&key)
                    .map_or_else(Vec::new, |play| play.close(reason, now))
            }
        } else {
            Vec::new()
        };
        drop(plays);
        emit(ux, lines);
    }

    pub fn expire(&self, now: SystemTime, ux: &Ux) {
        let mut plays = self.lock();
        let mut lines = Vec::new();
        for play in plays.values_mut() {
            for repeat in play.failures.values_mut() {
                if repeat.reported != repeat.count
                    && now
                        .duration_since(repeat.at)
                        .is_ok_and(|elapsed| elapsed >= REPEAT)
                {
                    lines.push(format!(
                        "{} count={} suppressed={}",
                        repeat.line, repeat.count, play.context.suppressed_requests
                    ));
                    repeat.reported = repeat.count;
                    repeat.at = now;
                }
            }
        }
        lines.extend(
            plays
                .extract_if(|_, play| {
                    play.active.is_empty()
                        && now
                            .duration_since(play.seen)
                            .is_ok_and(|duration| duration >= IDLE)
                })
                .flat_map(|(_, play)| play.close("idle", now)),
        );
        drop(plays);
        emit(ux, lines);
    }

    pub fn close_all(&self, now: SystemTime, ux: &Ux) {
        let lines = self
            .lock()
            .drain()
            .flat_map(|(_, play)| play.close("shutdown", now))
            .collect();
        emit(ux, lines);
    }
}
