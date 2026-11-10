use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::SystemTime;

use url::Url;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::log::Log;

use super::live::MediaInfo;
use crate::stats::RelayKind;

const SEGMENT_SLOW_HEADER_MS: i64 = 1500;
const SEGMENT_SLOW_TOTAL_MS: i64 = 2500;
const MANIFEST_SLOW_HEADER_MS: i64 = 2000;
const MANIFEST_SLOW_TOTAL_MS: i64 = 2500;
const SERVER_GENERATION_WAIT_MS: i64 = 1000;
const RETRY_WINDOW_MS: i64 = 15000;
const MAX_TRACKED_STREAMS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingSample {
    pub suppressed: bool,
    pub recovery: bool,
    pub retry_reason: Option<String>,
    pub request_id: String,
    pub stream_id: String,
    pub player_status: u16,
    pub playlist_shape: Option<String>,
    pub method: String,
    pub local_path: String,
    pub target_url: String,
    pub status: u16,
    pub header_ms: i64,
    pub total_ms: i64,
    pub bytes_out: u64,
    pub media: Option<MediaInfo>,
    pub lazy_state: Option<String>,
    pub lazy_wait_ms: i64,
    pub lazy_generator: Option<String>,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitchDiagnostic {
    pub kind: &'static str,
    pub reasons: String,
    pub stream_id: String,
    pub segment: i64,
    pub previous_segment: i64,
    pub gap_ms: i64,
    pub header_ms: i64,
    pub total_ms: i64,
}

#[derive(Default)]
pub struct HitchDetector {
    streams: Mutex<Streams>,
    notices: Mutex<Notices>,
}

#[derive(Default)]
struct Notices {
    entries: HashMap<String, Notice>,
    order: VecDeque<String>,
}

#[derive(Default)]
struct Notice {
    trimmed: Option<(SystemTime, u64, usize)>,
    slow: Option<SystemTime>,
    large: Option<SystemTime>,
}

#[derive(Default)]
struct Streams {
    state: HashMap<String, StreamState>,
    order: VecDeque<String>,
}

struct StreamState {
    last_segment: i64,
    last_seen: Option<SystemTime>,
}

impl HitchDetector {
    pub fn new() -> HitchDetector {
        HitchDetector::default()
    }

    pub fn record(&self, sample: &TimingSample, now: SystemTime, ux: &Ux, log: &Log) {
        let diagnostic = self.analyze(sample, now);
        self.record_diagnostic(sample, now, ux, log, diagnostic);
    }

    pub fn record_diagnostic(
        &self,
        sample: &TimingSample,
        now: SystemTime,
        ux: &Ux,
        log: &Log,
        diagnostic: Option<HitchDiagnostic>,
    ) {
        let (trim_notice, slow_notice, large_notice) = self.notices(sample, now);
        if trim_notice {
            let line = format!(
                "playlist trimmed{} host={} req={}",
                media_details(sample.media.as_ref()),
                describe_url(&sample.target_url),
                sample.request_id
            );
            if log.dev_console() {
                ux.write(Component::Relay, &line);
            } else {
                log.line(&format!("[relay] {line}"));
            }
        }
        let Some(mut d) = diagnostic else {
            return;
        };
        if !slow_notice && sample.media.is_some_and(|media| media.live) {
            d.reasons = d
                .reasons
                .split(',')
                .filter(|reason| {
                    !matches!(
                        *reason,
                        "slow-live-playlist-reload"
                            | "slow-manifest-total"
                            | "slow-manifest-headers"
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            if d.reasons.is_empty() {
                return;
            }
        }
        if !large_notice {
            d.reasons = d
                .reasons
                .split(',')
                .filter(|reason| *reason != "large-media-playlist")
                .collect::<Vec<_>>()
                .join(",");
            if d.reasons.is_empty() {
                return;
            }
        }
        let lazy = safe(sample.lazy_state.as_deref().unwrap_or("unknown"), 32);
        let file_line = format!(
            "[relay][hitch] reasons={} kind={} stream={}{}{} status={} lazy={lazy}{}{} header_ms={} total_ms={} bytes={}{}{} target={} req={} sent={}",
            d.reasons,
            d.kind,
            safe(&d.stream_id, 64),
            segment_part(d.segment),
            previous_part(d.previous_segment, d.gap_ms),
            sample.status,
            wait_part(sample.lazy_wait_ms),
            generator_part(sample.lazy_generator.as_deref()),
            d.header_ms,
            d.total_ms,
            sample.bytes_out,
            failure_part(sample.failure.as_deref()),
            media_details(sample.media.as_ref()),
            describe_url(&sample.target_url),
            sample.request_id,
            sample.player_status,
        );
        if log.dev_console() {
            ux.write(
                Component::Relay,
                file_line.strip_prefix("[relay]").unwrap_or(&file_line),
            );
        } else {
            log.line(&file_line);
        }
    }

    pub fn analyze(&self, sample: &TimingSample, now: SystemTime) -> Option<HitchDiagnostic> {
        if !sample.method.eq_ignore_ascii_case("GET") {
            return None;
        }
        let (kind, lazy) = classify(sample);
        let is_segment = kind == RelayKind::Segment;
        let is_manifest = kind == RelayKind::Manifest;

        let mut reasons: Vec<String> = Vec::new();
        let mut previous_segment = -1;
        let mut gap_ms = -1;
        let failure = sample.failure.as_deref().filter(|f| !f.trim().is_empty());
        if is_segment {
            if sample.status >= 400 {
                reasons.push(format!("segment-http-{}", sample.status));
            }
            if let Some(f) = failure {
                reasons.push(format!("segment-{f}"));
            }
            if sample.header_ms >= SEGMENT_SLOW_HEADER_MS {
                reasons.push("slow-upstream-headers".to_owned());
            }
            if sample.total_ms >= SEGMENT_SLOW_TOTAL_MS {
                reasons.push("slow-segment-total".to_owned());
            }
            if sample.lazy_wait_ms >= SERVER_GENERATION_WAIT_MS {
                reasons.push("server-generation-wait".to_owned());
            }
            if let Some((stream, segment)) = &lazy {
                (previous_segment, gap_ms) = self.sequence(stream, *segment, now, &mut reasons);
            }
        } else if is_manifest {
            if sample.status >= 400 {
                reasons.push(format!("manifest-http-{}", sample.status));
            }
            if let Some(f) = failure {
                reasons.push(format!("manifest-{f}"));
            }
            if sample.header_ms >= MANIFEST_SLOW_HEADER_MS {
                reasons.push("slow-manifest-headers".to_owned());
            }
            if sample.total_ms >= MANIFEST_SLOW_TOTAL_MS {
                reasons.push("slow-manifest-total".to_owned());
            }
            if let Some(media) = sample.media {
                if media.kept == media.segments
                    && (sample.bytes_out > 256 * 1024
                        || (media.segments > 120 && media.untrimmed == "budget"))
                {
                    reasons.push("large-media-playlist".to_owned());
                }
                if media.live
                    && media.target_ms > 0
                    && u64::try_from(sample.total_ms).is_ok_and(|total| total > media.target_ms / 2)
                {
                    reasons.push("slow-live-playlist-reload".to_owned());
                }
            }
        }
        if reasons.is_empty() {
            return None;
        }
        let (stream_id, segment) = match lazy {
            Some((s, n)) => (s, n),
            None => ("-".to_owned(), -1),
        };
        Some(HitchDiagnostic {
            kind: if is_segment { "segment" } else { "manifest" },
            reasons: reasons.join(","),
            stream_id,
            segment,
            previous_segment,
            gap_ms,
            header_ms: sample.header_ms,
            total_ms: sample.total_ms,
        })
    }

    fn notices(&self, sample: &TimingSample, now: SystemTime) -> (bool, bool, bool) {
        let Some(media) = sample
            .media
            .filter(|_| sample.method.eq_ignore_ascii_case("GET"))
        else {
            return (false, true, true);
        };
        let key = super::activity::play_id(&sample.local_path)
            .unwrap_or(&sample.local_path)
            .to_owned();
        let mut notices = self
            .notices
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !notices.entries.contains_key(&key) {
            if notices.entries.len() >= MAX_TRACKED_STREAMS
                && let Some(old) = notices.order.pop_front()
            {
                notices.entries.remove(&old);
            }
            notices.order.push_back(key.clone());
        }
        let notice = notices.entries.entry(key).or_default();
        let trimmed = media.kept < media.segments
            && notice.trimmed.is_none_or(|(at, bytes, kept)| {
                now.duration_since(at)
                    .is_ok_and(|elapsed| elapsed.as_secs() >= 600)
                    || bytes.abs_diff(media.bytes) >= (bytes / 4).max(256 * 1024)
                    || kept.abs_diff(media.kept) >= (kept / 4).max(1)
            });
        if trimmed {
            notice.trimmed = Some((now, media.bytes, media.kept));
        }
        let slow = media.live
            && (sample.header_ms >= MANIFEST_SLOW_HEADER_MS
                || sample.total_ms >= MANIFEST_SLOW_TOTAL_MS
                || (media.target_ms > 0
                    && u64::try_from(sample.total_ms)
                        .is_ok_and(|total| total > media.target_ms / 2)));
        let report_slow = !slow
            || notice.slow.is_none_or(|at| {
                now.duration_since(at)
                    .is_ok_and(|elapsed| elapsed.as_secs() >= 60)
            });
        if slow && report_slow {
            notice.slow = Some(now);
        }
        let large = media.kept == media.segments
            && (sample.bytes_out > 256 * 1024
                || (media.segments > 120 && media.untrimmed == "budget"));
        let report_large = !large
            || notice.large.is_none_or(|at| {
                now.duration_since(at)
                    .is_ok_and(|elapsed| elapsed.as_secs() >= 60)
            });
        if large && report_large {
            notice.large = Some(now);
        }
        drop(notices);
        (trimmed, report_slow, report_large)
    }

    fn sequence(
        &self,
        stream: &str,
        segment: i64,
        now: SystemTime,
        reasons: &mut Vec<String>,
    ) -> (i64, i64) {
        if stream.trim().is_empty() || segment < 0 {
            return (-1, -1);
        }
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        if !streams.state.contains_key(stream) {
            streams.order.push_back(stream.to_owned());
            streams.state.insert(
                stream.to_owned(),
                StreamState {
                    last_segment: -1,
                    last_seen: None,
                },
            );
            while streams.state.len() > MAX_TRACKED_STREAMS {
                let Some(old) = streams.order.pop_front() else {
                    break;
                };
                streams.state.remove(&old);
            }
        }
        let Some(state) = streams.state.get_mut(stream) else {
            return (-1, -1);
        };
        let previous = state.last_segment;
        let gap = state.last_seen.map_or(-1, |seen| {
            now.duration_since(seen)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        });
        if previous == segment && (0..=RETRY_WINDOW_MS).contains(&gap) {
            reasons.push("segment-retry".to_owned());
        } else if previous >= 0 && segment > previous + 1 {
            reasons.push("segment-skip".to_owned());
        } else if previous >= 0 && segment < previous {
            reasons.push("segment-backtrack".to_owned());
        }
        state.last_segment = segment;
        state.last_seen = Some(now);
        drop(streams);
        (previous, gap)
    }
}

pub(crate) fn media_details(media: Option<&MediaInfo>) -> String {
    media.map_or_else(String::new, |media| {
        format!(
            " playlist-bytes={} segments={} trimmed={} kept={}{}",
            media.bytes,
            media.segments,
            media.kept < media.segments,
            media.kept,
            if media.kept == media.segments && !media.untrimmed.is_empty() {
                format!(" untrimmed={}", media.untrimmed)
            } else {
                String::new()
            },
        )
    })
}

pub fn relay_kind(sample: &TimingSample) -> RelayKind {
    classify(sample).0
}

fn classify(sample: &TimingSample) -> (RelayKind, Option<(String, i64)>) {
    let parsed = Url::parse(&sample.target_url).ok();
    let path = parsed
        .as_ref()
        .map_or(sample.target_url.as_str(), |u| u.path());
    let lazy = parsed.as_ref().and_then(|u| parse_lazy_path(u.path()));
    let kind = if lazy.is_some()
        || ends_with_segment_ext(path)
        || ends_with_segment_ext(&sample.local_path)
    {
        RelayKind::Segment
    } else if ends_with_manifest_ext(path) || ends_with_manifest_ext(&sample.local_path) {
        RelayKind::Manifest
    } else {
        RelayKind::Other
    };
    (kind, lazy)
}

pub fn parse_lazy_segment(url: &str) -> Option<(String, i64)> {
    let u = Url::parse(url).ok()?;
    parse_lazy_path(u.path())
}

fn parse_lazy_path(path: &str) -> Option<(String, i64)> {
    if !path.to_ascii_lowercase().contains("lazy-hls") {
        return None;
    }
    let parts: Vec<&str> = path
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    for i in 0..parts.len().saturating_sub(2) {
        if !parts[i].eq_ignore_ascii_case("lazy-hls") {
            continue;
        }
        if let Some(segment) = parts[i + 2..].iter().find_map(|p| parse_segment_file(p)) {
            return Some((parts[i + 1].to_owned(), segment));
        }
    }
    None
}

fn parse_segment_file(name: &str) -> Option<i64> {
    let rest = name
        .get(..4)
        .filter(|p| p.eq_ignore_ascii_case("seg_"))
        .map(|_| &name[4..])?;
    let digits: &str = &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()];
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i32>().ok().map(i64::from)
}

fn ends_with_segment_ext(value: &str) -> bool {
    [".ts", ".m4s", ".mp4"]
        .iter()
        .any(|e| ends_with_ignore_case(value, e))
}

fn ends_with_manifest_ext(value: &str) -> bool {
    [".m3u8", ".mpd"]
        .iter()
        .any(|e| ends_with_ignore_case(value, e))
}

fn ends_with_ignore_case(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.is_char_boundary(s.len() - suffix.len())
        && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn describe_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned())
}

fn segment_part(segment: i64) -> String {
    if segment >= 0 {
        format!(" segment={segment}")
    } else {
        String::new()
    }
}

fn previous_part(previous: i64, gap_ms: i64) -> String {
    if previous < 0 {
        return String::new();
    }
    format!(" previous_segment={previous} gap_ms={gap_ms}")
}

fn wait_part(wait_ms: i64) -> String {
    if wait_ms >= 0 {
        format!(" lazy_wait_ms={wait_ms}")
    } else {
        String::new()
    }
}

fn generator_part(generator: Option<&str>) -> String {
    match generator.filter(|g| !g.trim().is_empty()) {
        Some(g) => format!(" lazy_generator={}", safe(g, 32)),
        None => String::new(),
    }
}

fn failure_part(failure: Option<&str>) -> String {
    match failure.filter(|f| !f.trim().is_empty()) {
        Some(f) => format!(" failure={}", safe(f, 64)),
        None => String::new(),
    }
}

fn safe(value: &str, max: usize) -> String {
    crate::diagnostics::token(value).chars().take(max).collect()
}
