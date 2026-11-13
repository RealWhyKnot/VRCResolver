use super::*;

impl PlaybackLog {
    pub fn reset_video_failure(&self, url: &str) {
        self.video_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&url_key(url));
    }

    pub fn first_video_failure(&self, url: &str, now: SystemTime) -> bool {
        if url.is_empty() {
            return true;
        }
        let mut failures = self
            .video_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        failures.retain(|_, at| now.duration_since(*at).map_or(true, |age| age < IDLE));
        let key = url_key(url);
        if failures.contains_key(&key) {
            return false;
        }
        if failures.len() >= CAPACITY
            && let Some(oldest) = failures
                .iter()
                .min_by_key(|(_, at)| *at)
                .map(|(key, _)| key.clone())
        {
            failures.remove(&oldest);
        }
        failures.insert(key, now);
        true
    }
}
