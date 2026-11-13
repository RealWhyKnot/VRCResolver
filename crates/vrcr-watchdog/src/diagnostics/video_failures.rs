use super::*;

#[derive(Default)]
pub(super) struct VideoFailures {
    entries: Vec<VideoFailure>,
}

struct VideoFailure {
    aliases: HashSet<String>,
    seen: SystemTime,
    reported: bool,
}

fn key(url: &str) -> String {
    url_key(&vrcr_shared::trust_gateway::extract_target(url).unwrap_or_else(|| url.to_owned()))
}

impl VideoFailures {
    fn entry(&mut self, url: &str, now: SystemTime) -> &mut VideoFailure {
        self.entries.retain(|entry| {
            now.duration_since(entry.seen)
                .map_or(true, |age| age < IDLE)
        });
        let key = key(url);
        let index = if let Some(index) = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.aliases.contains(&key))
            .max_by_key(|(_, entry)| entry.seen)
            .map(|(index, _)| index)
        {
            index
        } else {
            if self.entries.len() >= CAPACITY {
                let oldest = self
                    .entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, entry)| entry.seen)
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                self.entries.remove(oldest);
            }
            self.entries.push(VideoFailure {
                aliases: HashSet::from([key]),
                seen: now,
                reported: false,
            });
            self.entries.len() - 1
        };
        let entry = &mut self.entries[index];
        entry.seen = now;
        entry
    }
}

impl PlaybackLog {
    pub fn reset_video_failure(&self, url: &str) {
        let mut failures = self
            .video_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let key = key(url);
        for entry in &mut failures.entries {
            if entry.aliases.contains(&key) {
                entry.reported = false;
            }
        }
    }

    pub(super) fn remember_video_source(&self, source: &str, resolved: &str, now: SystemTime) {
        let mut failures = self
            .video_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = failures.entry(source, now);
        entry.aliases = HashSet::from([key(source), key(resolved)]);
        drop(failures);
    }

    pub fn first_video_failure(&self, url: &str, now: SystemTime) -> bool {
        if url.is_empty() {
            return true;
        }
        let mut failures = self
            .video_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = failures.entry(url, now);
        let first = !std::mem::replace(&mut entry.reported, true);
        drop(failures);
        first
    }
}
