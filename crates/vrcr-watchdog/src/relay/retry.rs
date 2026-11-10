use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};
use sha2::{Digest, Sha256};

pub type Invalidator = Arc<dyn Fn(&str) + Send + Sync>;
pub type Reporter = Arc<dyn Fn(crate::ipc::Feedback) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Hooks {
    pub invalidate: Option<Invalidator>,
    pub report: Option<Reporter>,
}

pub const CAPACITY: usize = 256;
pub const MAX_BODY: usize = 8192;
pub const RECREATE: &str = "stream_recreate_required";

#[derive(Clone)]
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub reason: &'static str,
    pub until: SystemTime,
}

#[derive(Default)]
pub struct RetryMemory {
    answers: Mutex<HashMap<String, Answer>>,
    recent: Mutex<HashMap<String, (String, SystemTime, bool)>>,
    retired: Mutex<HashMap<String, SystemTime>>,
}

pub fn key(playback: &str, target: &str, method: &Method, headers: &HeaderMap) -> String {
    let mut hash = Sha256::new();
    hash.update(playback.as_bytes());
    hash.update([0]);
    hash.update(target.as_bytes());
    hash.update([0]);
    hash.update(method.as_str().as_bytes());
    let mut parts: Vec<_> = headers
        .iter()
        .filter(|(name, _)| name.as_str() != "x-request-id")
        .collect();
    parts.sort_by(|(a, av), (b, bv)| {
        a.as_str()
            .cmp(b.as_str())
            .then(av.as_bytes().cmp(bv.as_bytes()))
    });
    for (name, value) in parts {
        hash.update([0]);
        hash.update(name.as_str().as_bytes());
        hash.update([0]);
        hash.update(value.as_bytes());
    }
    hex::encode(hash.finalize())
}

pub fn reason(headers: &HeaderMap, body: &[u8]) -> &'static str {
    fn known(value: &str) -> Option<&'static str> {
        match value.trim() {
            RECREATE => Some(RECREATE),
            "unknown_stream" | "stream_unknown" | "stream_not_found" | "unknown stream"
            | "stream not found" => Some("unknown_stream"),
            _ => None,
        }
    }
    fn metadata(value: &str) -> Option<&'static str> {
        known(value).or_else(|| {
            (!value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)))
            .then_some("upstream_failure")
        })
    }
    if let Some(value) = headers.get("x-vrcr-reason") {
        return value
            .to_str()
            .ok()
            .and_then(metadata)
            .unwrap_or("upstream_status");
    }
    for name in ["x-error-reason", "x-vrcresolver-reason"] {
        if let Some(reason) = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(known)
        {
            return reason;
        }
    }
    if body.len() <= MAX_BODY {
        if let Ok(text) = std::str::from_utf8(body)
            && let Some(reason) = known(text)
        {
            return reason;
        }
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body)
            && let Some(reason) = value
                .get("reason")
                .and_then(|v| v.as_str())
                .and_then(metadata)
        {
            return reason;
        }
    }
    "upstream_status"
}

pub fn delay(status: u16, reason: &str, headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    if matches!(status, 404 | 410) && matches!(reason, "unknown_stream" | RECREATE) {
        return Some(Duration::from_secs(5));
    }
    if !matches!(status, 429 | 502 | 503 | 504) {
        return None;
    }
    let fallback = 1;
    let seconds = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .map_or(fallback, |value| {
            if value.starts_with('-') && value[1..].bytes().all(|v| v.is_ascii_digit()) {
                return 1;
            }
            if !value.is_empty() && value.bytes().all(|v| v.is_ascii_digit()) {
                return value.parse::<u64>().unwrap_or(30).clamp(1, 30);
            }
            httpdate::parse_http_date(value)
                .ok()
                .map_or(fallback, |date| {
                    let remaining = date.duration_since(now).unwrap_or_default();
                    remaining
                        .as_secs()
                        .saturating_add(u64::from(remaining.subsec_nanos() != 0))
                        .clamp(1, 30)
                })
        });
    Some(Duration::from_secs(seconds))
}

impl RetryMemory {
    pub fn get(&self, key: &str, now: SystemTime) -> Option<Answer> {
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        answers.retain(|_, answer| answer.until > now);
        answers.get(key).cloned().map(|mut answer| {
            let remaining = answer.until.duration_since(now).unwrap_or_default();
            let seconds = remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() > 0))
                .max(1);
            answer
                .headers
                .insert("retry-after", http::HeaderValue::from(seconds));
            answer
        })
    }

    pub fn confirmed(&self, key: &str, answer: &Answer, now: SystemTime) -> bool {
        if !matches!(answer.status.as_u16(), 502..=504)
            || answer.headers.contains_key("retry-after")
            || answer.reason != "upstream_status"
        {
            return true;
        }
        let mut digest = Sha256::new();
        digest.update(answer.status.as_u16().to_be_bytes());
        digest.update(&answer.body);
        let fingerprint = hex::encode(digest.finalize());
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        recent.retain(|_, (_, at, confirmed)| {
            now.duration_since(*at)
                .is_ok_and(|elapsed| elapsed < Duration::from_secs(if *confirmed { 30 } else { 1 }))
        });
        let confirmed = recent
            .get(key)
            .is_some_and(|(previous, _, _)| *previous == fingerprint);
        if recent.len() >= CAPACITY
            && !recent.contains_key(key)
            && let Some(oldest) = recent
                .iter()
                .min_by_key(|(_, (_, at, _))| *at)
                .map(|(key, _)| key.clone())
        {
            recent.remove(&oldest);
        }
        recent.insert(key.to_owned(), (fingerprint, now, confirmed));
        confirmed
    }

    pub fn put(&self, key: String, answer: Answer, now: SystemTime) {
        let size: usize = answer
            .headers
            .iter()
            .map(|(k, v)| k.as_str().len() + v.as_bytes().len())
            .sum();
        if answer.body.len() > MAX_BODY || size > 16384 {
            return;
        }
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        answers.retain(|_, answer| answer.until > now);
        if answers.len() >= CAPACITY
            && !answers.contains_key(&key)
            && let Some(oldest) = answers
                .iter()
                .min_by_key(|(_, v)| v.until)
                .map(|(k, _)| k.clone())
        {
            answers.remove(&oldest);
        }
        answers.insert(key, answer);
    }

    pub fn clear(&self, key: &str) {
        self.recent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        self.answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    pub fn len(&self) -> usize {
        self.answers.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_retired(&self, playback: &str, now: SystemTime) -> bool {
        let mut retired = self.retired.lock().unwrap_or_else(|e| e.into_inner());
        retired.retain(|_, until| *until > now);
        retired.contains_key(playback)
    }

    pub fn retire(&self, playback: &str, now: SystemTime) -> bool {
        let mut retired = self.retired.lock().unwrap_or_else(|e| e.into_inner());
        retired.retain(|_, until| *until > now);
        if retired.contains_key(playback) {
            return false;
        }
        if retired.len() >= CAPACITY
            && let Some(oldest) = retired
                .iter()
                .min_by_key(|(_, until)| **until)
                .map(|(key, _)| key.clone())
        {
            retired.remove(&oldest);
        }
        retired.insert(playback.to_owned(), now + Duration::from_secs(300));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_retirement_and_flight_registries_stay_bounded_and_expire() {
        let memory = RetryMemory::default();
        let now = SystemTime::UNIX_EPOCH;
        for index in 0..10_000 {
            let key = index.to_string();
            memory.retire(&key, now);
            assert!(memory.retired.lock().expect("retired").len() <= CAPACITY);
        }
        assert!(memory.is_retired("9999", now + Duration::from_secs(299)));
        assert!(!memory.is_retired("9999", now + Duration::from_secs(300)));
        assert!(!memory.is_retired("9999", now + Duration::from_secs(600)));
        assert_eq!(memory.retired.lock().expect("retired").len(), 0);
    }
}
