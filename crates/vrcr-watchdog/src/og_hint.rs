use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use vrcr_shared::logutil;

use crate::clock::{self, Now};

pub const DEFAULT_TTL: Duration = Duration::from_secs(60);
pub const OG_BLOCKED_TTL: Duration = Duration::from_secs(30 * 60);

pub struct OgFallbackHint {
    ttl: Duration,
    now: Now,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    expires: HashMap<String, SystemTime>,
    blocked_until: HashMap<String, SystemTime>,
}

impl Default for OgFallbackHint {
    fn default() -> Self {
        OgFallbackHint::new(DEFAULT_TTL, clock::system())
    }
}

impl OgFallbackHint {
    pub fn new(ttl: Duration, now: Now) -> OgFallbackHint {
        OgFallbackHint {
            ttl,
            now,
            state: Mutex::new(State::default()),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn record_load_failure(&self, source_url: &str) {
        if source_url.is_empty() {
            return;
        }
        let until = (self.now)() + self.ttl;
        self.state().expires.insert(source_url.to_owned(), until);
    }

    pub fn should_prefer_og(&self, source_url: &str) -> bool {
        if source_url.is_empty() || self.is_og_blocked(source_url) {
            return false;
        }
        let now = (self.now)();
        let mut st = self.state();
        match st.expires.get(source_url) {
            None => false,
            Some(exp) if *exp > now => true,
            Some(_) => {
                st.expires.remove(source_url);
                false
            }
        }
    }

    pub fn try_clear(&self, source_url: &str) -> bool {
        !source_url.is_empty() && self.state().expires.remove(source_url).is_some()
    }

    pub fn record_og_blocked(&self, source_url: &str) -> bool {
        let site = site_key(source_url);
        if site.is_empty() {
            return false;
        }
        let was_blocked = self.is_og_blocked(source_url);
        let until = (self.now)() + OG_BLOCKED_TTL;
        self.state().blocked_until.insert(site, until);
        !was_blocked
    }

    pub fn is_og_blocked(&self, source_url: &str) -> bool {
        let site = site_key(source_url);
        if site.is_empty() {
            return false;
        }
        let now = (self.now)();
        let mut st = self.state();
        match st.blocked_until.get(&site) {
            None => false,
            Some(until) if *until > now => true,
            Some(_) => {
                st.blocked_until.remove(&site);
                false
            }
        }
    }

    pub fn live_entry_count(&self) -> usize {
        let now = (self.now)();
        self.state().expires.values().filter(|e| **e > now).count()
    }
}

pub fn site_key(url: &str) -> String {
    let host = logutil::bare_host(url).to_lowercase();
    if host == "?" || host.is_empty() {
        return String::new();
    }
    if matches!(host.as_str(), "youtu.be" | "youtube.com" | "youtube-nocookie.com")
        || host.ends_with(".youtube.com")
    {
        return "youtube.com".to_owned();
    }
    host
}
