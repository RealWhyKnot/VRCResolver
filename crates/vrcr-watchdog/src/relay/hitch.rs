use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::SystemTime;

use url::Url;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::logutil::sanitize_for_console;

const SEGMENT_SLOW_HEADER_MS: i64 = 1500;
const SEGMENT_SLOW_TOTAL_MS: i64 = 2500;
const MANIFEST_SLOW_HEADER_MS: i64 = 2000;
const MANIFEST_SLOW_TOTAL_MS: i64 = 2500;
const SERVER_GENERATION_WAIT_MS: i64 = 1000;
const RETRY_WINDOW_MS: i64 = 15000;
const MAX_TRACKED_STREAMS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingSample {
    pub method: String,
    pub local_path: String,
    pub target_url: String,
    pub status: u16,
    pub header_ms: i64,
    pub total_ms: i64,
    pub bytes_out: u64,
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
        let Some(d) = self.analyze(sample, now) else {
            return;
        };
        let lazy = safe(sample.lazy_state.as_deref().unwrap_or("unknown"), 32);
        let file_line = format!(
            "[relay][hitch] reasons={} kind={} stream={}{}{} status={} lazy={lazy}{}{} header_ms={} total_ms={} bytes={}{} target={}",
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
            describe_url(&sample.target_url),
        );
        if log.dev_console() {
            ux.warn(
                Component::Relay,
                &format!(
                    "hitch reasons={} kind={}{} status={}{}{} total_ms={}{}",
                    d.reasons,
                    d.kind,
                    segment_part(d.segment),
                    sample.status,
                    wait_part(sample.lazy_wait_ms),
                    generator_part(sample.lazy_generator.as_deref()),
                    d.total_ms,
                    failure_part(sample.failure.as_deref()),
                ),
            );
        } else {
            log.line(&file_line);
        }
    }

    pub fn analyze(&self, sample: &TimingSample, now: SystemTime) -> Option<HitchDiagnostic> {
        if !sample.method.eq_ignore_ascii_case("GET") {
            return None;
        }
        let parsed = Url::parse(&sample.target_url).ok();
        let path = parsed.as_ref().map_or(sample.target_url.as_str(), |u| u.path());
        let lazy = parsed.as_ref().and_then(|u| parse_lazy_path(u.path()));
        let is_segment =
            lazy.is_some() || ends_with_segment_ext(path) || ends_with_segment_ext(&sample.local_path);
        let is_manifest =
            !is_segment && (ends_with_manifest_ext(path) || ends_with_manifest_ext(&sample.local_path));

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

    fn sequence(&self, stream: &str, segment: i64, now: SystemTime, reasons: &mut Vec<String>) -> (i64, i64) {
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
        (previous, gap)
    }
}

pub fn parse_lazy_segment(url: &str) -> Option<(String, i64)> {
    let u = Url::parse(url).ok()?;
    parse_lazy_path(u.path())
}

fn parse_lazy_path(path: &str) -> Option<(String, i64)> {
    if !path.to_ascii_lowercase().contains("lazy-hls") {
        return None;
    }
    let parts: Vec<&str> = path.split('/').map(str::trim).filter(|p| !p.is_empty()).collect();
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
    [".m3u8", ".mpd"].iter().any(|e| ends_with_ignore_case(value, e))
}

fn ends_with_ignore_case(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.is_char_boundary(s.len() - suffix.len())
        && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn describe_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(u) => safe(&format!("{}{}", u.host_str().unwrap_or_default(), u.path()), 140),
        Err(_) => safe(url, 120),
    }
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
    sanitize_for_console(value, max)
}
