use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::MediaInfo;

const CAPACITY: usize = 256;
const EXPIRY: Duration = Duration::from_secs(300);

#[derive(Default)]
pub(crate) struct History {
    entries: Mutex<HashMap<String, Instant>>,
}

pub(crate) fn identity(target: &str) -> String {
    let key = url::Url::parse(target).map_or_else(
        |_| target.to_owned(),
        |url| format!("{}{}", url.origin().ascii_serialization(), url.path()),
    );
    super::super::manifest::target_namespace(&key)
}

impl History {
    pub(crate) fn contains(&self, key: &str, now: Instant) -> bool {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|_, seen| now.saturating_duration_since(*seen) < EXPIRY);
        let found = if let Some(seen) = entries.get_mut(key) {
            *seen = now;
            true
        } else {
            false
        };
        drop(entries);
        found
    }

    pub(crate) fn observe(&self, key: String, media: Option<MediaInfo>, now: Instant) {
        let Some(media) = media else { return };
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|_, seen| now.saturating_duration_since(*seen) < EXPIRY);
        if !media.live {
            entries.remove(&key);
        } else if media.needs_trim || media.segments > 120 || media.kept < media.segments {
            if entries.len() >= CAPACITY
                && !entries.contains_key(&key)
                && let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, seen)| **seen)
                    .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
            entries.insert(key, now);
        }
    }
}

#[cfg(test)]
mod tests;
