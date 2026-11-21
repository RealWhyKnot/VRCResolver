use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use vrcr_shared::source_url;

use crate::clock::{self, Now};

pub const LEASES: [Duration; 3] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
];
pub const STRIKE_DEBOUNCE: Duration = Duration::from_secs(60);
pub const STRIKE_MEMORY: Duration = Duration::from_secs(2 * 60 * 60);
pub const OG_BLOCKED_TTL: Duration = Duration::from_secs(30 * 60);
pub const MAX_ENTRIES: usize = 512;

pub struct OgFallbackHint {
    now: Now,
    state: Mutex<State>,
}

#[derive(Clone, Copy)]
struct Entry {
    until: SystemTime,
    strikes: usize,
    last_strike: SystemTime,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    blocked_until: HashMap<String, SystemTime>,
}

impl Default for OgFallbackHint {
    fn default() -> Self {
        OgFallbackHint::new(clock::system())
    }
}

pub fn lease_for(strikes: usize) -> Duration {
    LEASES[strikes.clamp(1, LEASES.len()) - 1]
}

fn key(source_url: &str) -> Option<String> {
    let trimmed = source_url.trim();
    (!trimmed.is_empty()).then(|| source_url::normalize(trimmed))
}

impl OgFallbackHint {
    pub fn new(now: Now) -> OgFallbackHint {
        OgFallbackHint {
            now,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn record_load_failure(&self, source_url: &str) -> Option<Duration> {
        let key = key(source_url)?;
        let now = (self.now)();
        let mut st = self.state();
        let entry = st.entries.entry(key).or_insert(Entry {
            until: now,
            strikes: 0,
            last_strike: now,
        });
        if entry.strikes > 0 && elapsed(entry.last_strike, now) >= STRIKE_MEMORY {
            entry.strikes = 0;
        }
        if entry.strikes == 0 || elapsed(entry.last_strike, now) >= STRIKE_DEBOUNCE {
            entry.strikes += 1;
            entry.last_strike = now;
        }
        let lease = lease_for(entry.strikes);
        entry.until = entry.until.max(now + lease);
        prune(&mut st, now);
        drop(st);
        Some(lease)
    }

    pub fn should_prefer_og(&self, source_url: &str) -> bool {
        !self.is_og_blocked(source_url) && self.remaining(source_url).is_some()
    }

    pub fn remaining(&self, source_url: &str) -> Option<Duration> {
        let key = key(source_url)?;
        let now = (self.now)();
        let until = self.state().entries.get(&key)?.until;
        until.duration_since(now).ok().filter(|d| !d.is_zero())
    }

    pub fn strikes(&self, source_url: &str) -> usize {
        key(source_url)
            .and_then(|k| self.state().entries.get(&k).map(|e| e.strikes))
            .unwrap_or(0)
    }

    pub fn try_clear(&self, source_url: &str) -> bool {
        let Some(key) = key(source_url) else {
            return false;
        };
        let now = (self.now)();
        let mut st = self.state();
        st.blocked_until.remove(&key);
        st.entries.remove(&key).is_some_and(|e| e.until > now)
    }

    pub fn record_og_blocked(&self, source_url: &str) -> bool {
        let Some(key) = key(source_url).filter(|k| has_host(k)) else {
            return false;
        };
        let was_blocked = self.is_og_blocked(source_url);
        let now = (self.now)();
        let mut st = self.state();
        st.blocked_until.insert(key, now + OG_BLOCKED_TTL);
        prune(&mut st, now);
        drop(st);
        !was_blocked
    }

    pub fn is_og_blocked(&self, source_url: &str) -> bool {
        let Some(key) = key(source_url) else {
            return false;
        };
        let now = (self.now)();
        let mut st = self.state();
        match st.blocked_until.get(&key) {
            None => false,
            Some(until) if *until > now => true,
            Some(_) => {
                st.blocked_until.remove(&key);
                false
            }
        }
    }

    pub fn live_entry_count(&self) -> usize {
        let now = (self.now)();
        self.state()
            .entries
            .values()
            .filter(|e| e.until > now)
            .count()
    }
}

fn has_host(key: &str) -> bool {
    url::Url::parse(key).is_ok_and(|u| u.host_str().is_some_and(|h| !h.is_empty()))
}

fn elapsed(since: SystemTime, now: SystemTime) -> Duration {
    now.duration_since(since).unwrap_or_default()
}

fn prune(st: &mut State, now: SystemTime) {
    st.blocked_until.retain(|_, until| *until > now);
    if st.entries.len() <= MAX_ENTRIES {
        return;
    }
    st.entries
        .retain(|_, e| e.until > now || elapsed(e.last_strike, now) < STRIKE_MEMORY);
    while st.entries.len() > MAX_ENTRIES {
        let Some(oldest) = st
            .entries
            .iter()
            .min_by_key(|(_, e)| (e.until, e.last_strike))
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        st.entries.remove(&oldest);
    }
}

pub fn arm(
    cache: Option<&crate::resolve_cache::ResolveCache>,
    hint: Option<&OgFallbackHint>,
    source_url: &str,
) -> (usize, Option<Duration>) {
    if source_url.trim().is_empty() {
        return (0, None);
    }
    let lease = hint.and_then(|h| h.record_load_failure(source_url));
    let evicted = cache.map_or(0, |c| c.evict_by_url(source_url));
    (evicted, lease)
}

pub fn lease_label(lease: Option<Duration>) -> String {
    lease.map_or_else(|| "off".to_owned(), |l| format!("{}m", l.as_secs() / 60))
}
