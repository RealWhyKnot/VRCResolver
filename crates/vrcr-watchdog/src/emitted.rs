use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use vrcr_shared::source_url;
use vrcr_shared::trust_gateway::extract_target;

use crate::clock::{self, Now};

pub const MAX_ENTRIES: usize = 256;
pub const MAX_IDS: usize = 256;
pub const ENTRY_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub source_url: String,
    pub correlation_id: Option<String>,
}

struct Entry {
    key: String,
    normalized: String,
    origin: Origin,
    at: SystemTime,
}

struct IdEntry {
    request_id: String,
    origin: Origin,
    at: SystemTime,
}

#[derive(Default)]
struct State {
    urls: VecDeque<Entry>,
    ids: VecDeque<IdEntry>,
}

pub struct EmittedUrls {
    now: Now,
    state: Mutex<State>,
}

impl Default for EmittedUrls {
    fn default() -> Self {
        EmittedUrls::new(clock::system())
    }
}

impl EmittedUrls {
    pub fn new(now: Now) -> EmittedUrls {
        EmittedUrls {
            now,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn remember(&self, emitted: &str, origin: &Origin) {
        let key = emitted.trim();
        if key.is_empty() || origin.source_url.trim().is_empty() {
            return;
        }
        let now = (self.now)();
        let mut state = self.state();
        state.urls.retain(|e| e.key != key && !expired(e.at, now));
        state.urls.push_back(Entry {
            key: key.to_owned(),
            normalized: source_url::normalize(key),
            origin: origin.clone(),
            at: now,
        });
        while state.urls.len() > MAX_ENTRIES {
            state.urls.pop_front();
        }
        drop(state);
    }

    pub fn remember_id(&self, request_id: &str, origin: &Origin) {
        let request_id = request_id.trim();
        if request_id.is_empty() || origin.source_url.trim().is_empty() {
            return;
        }
        let now = (self.now)();
        let mut state = self.state();
        state
            .ids
            .retain(|e| e.request_id != request_id && !expired(e.at, now));
        state.ids.push_back(IdEntry {
            request_id: request_id.to_owned(),
            origin: origin.clone(),
            at: now,
        });
        while state.ids.len() > MAX_IDS {
            state.ids.pop_front();
        }
        drop(state);
    }

    pub fn origin_by_id(&self, id: &str) -> Option<Origin> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let now = (self.now)();
        let state = self.state();
        let live = || state.ids.iter().rev().filter(|e| !expired(e.at, now));
        live()
            .find(|e| e.request_id == id)
            .or_else(|| live().find(|e| e.origin.correlation_id.as_deref() == Some(id)))
            .map(|e| e.origin.clone())
    }

    pub fn origin_by_url(&self, emitted: &str) -> Option<Origin> {
        let key = emitted.trim();
        if key.is_empty() {
            return None;
        }
        let target = extract_target(key);
        let normalized = source_url::normalize(target.as_deref().unwrap_or(key));
        let now = (self.now)();
        let state = self.state();
        let live = || state.urls.iter().rev().filter(|e| !expired(e.at, now));
        live()
            .find(|e| e.key == key)
            .or_else(|| target.as_deref().and_then(|t| live().find(|e| e.key == t)))
            .or_else(|| live().find(|e| e.normalized == normalized))
            .map(|e| e.origin.clone())
    }

    pub fn origin_for_revocation(
        &self,
        request_id: &str,
        correlation_id: Option<&str>,
        url: Option<&str>,
    ) -> Option<Origin> {
        self.origin_by_id(request_id)
            .or_else(|| correlation_id.and_then(|cid| self.origin_by_id(cid)))
            .or_else(|| url.and_then(|u| self.origin_by_url(u)))
            .or_else(|| {
                url.filter(|u| !u.trim().is_empty()).map(|u| Origin {
                    source_url: u.to_owned(),
                    correlation_id: correlation_id.map(str::to_owned),
                })
            })
    }

    pub fn len(&self) -> usize {
        self.state().urls.len()
    }

    pub fn id_count(&self) -> usize {
        self.state().ids.len()
    }

    pub fn is_empty(&self) -> bool {
        let state = self.state();
        state.urls.is_empty() && state.ids.is_empty()
    }
}

fn expired(at: SystemTime, now: SystemTime) -> bool {
    now.duration_since(at).is_ok_and(|age| age > ENTRY_TTL)
}
