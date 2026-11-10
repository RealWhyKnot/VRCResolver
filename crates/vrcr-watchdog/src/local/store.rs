use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use url::Url;

pub const MANIFEST_TTL: Duration = Duration::from_secs(5 * 60 * 60);
pub const MAX_MANIFESTS: usize = 32;
pub const FAILURES_BEFORE_REST: u32 = 2;
pub const FAULTS_BEFORE_REST: u32 = 3;
pub const REST: Duration = Duration::from_secs(30 * 60);

struct Entry {
    token: String,
    body: String,
    source: String,
    expires: Instant,
}

#[derive(Default)]
struct State {
    entries: VecDeque<Entry>,
    failures: u32,
    faults: u32,
    resting_until: Option<Instant>,
}

#[derive(Default)]
pub struct Manifests {
    state: Mutex<State>,
}

impl Manifests {
    pub fn insert(&self, body: String, source: &str) -> String {
        self.insert_at(body, source, Instant::now())
    }

    pub fn get(&self, token: &str) -> Option<String> {
        self.get_at(token, Instant::now())
    }

    pub fn is_resting(&self) -> bool {
        self.is_resting_at(Instant::now())
    }

    pub fn failed(&self, url: &str) -> Option<String> {
        self.failed_at(url, Instant::now())
    }

    pub fn fault(&self) {
        self.fault_at(Instant::now());
    }

    pub fn fault_at(&self, now: Instant) {
        let mut state = self.state();
        state.faults += 1;
        if state.faults >= FAULTS_BEFORE_REST {
            state.faults = 0;
            state.resting_until = Some(now + REST);
        }
    }

    pub fn answered(&self) {
        self.state().faults = 0;
    }

    pub fn played_elsewhere(&self) {
        self.state().failures = 0;
    }

    pub fn insert_at(&self, body: String, source: &str, now: Instant) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut state = self.state();
        state.entries.retain(|entry| entry.expires > now);
        while state.entries.len() >= MAX_MANIFESTS {
            state.entries.pop_front();
        }
        state.entries.push_back(Entry {
            token: token.clone(),
            body,
            source: source.to_owned(),
            expires: now + MANIFEST_TTL,
        });
        token
    }

    pub fn get_at(&self, token: &str, now: Instant) -> Option<String> {
        self.state()
            .entries
            .iter()
            .find(|entry| entry.token == token && entry.expires > now)
            .map(|entry| entry.body.clone())
    }

    pub fn is_resting_at(&self, now: Instant) -> bool {
        self.state().resting_until.is_some_and(|until| now < until)
    }

    pub fn source(&self, url: &str) -> Option<String> {
        let token = url_token(url)?;
        self.state()
            .entries
            .iter()
            .find(|entry| entry.token == token)
            .map(|entry| entry.source.clone())
    }

    pub fn failed_at(&self, url: &str, now: Instant) -> Option<String> {
        let token = url_token(url)?;
        let mut state = self.state();
        let index = state
            .entries
            .iter()
            .position(|entry| entry.token == token)?;
        let source = state.entries.remove(index)?.source;
        state.failures += 1;
        if state.failures >= FAILURES_BEFORE_REST {
            state.failures = 0;
            state.resting_until = Some(now + REST);
        }
        drop(state);
        Some(source)
    }

    pub fn confirmed(&self, url: &str) -> bool {
        let Some(token) = url_token(url) else {
            return false;
        };
        let mut state = self.state();
        let is_known = state.entries.iter().any(|entry| entry.token == token);
        if is_known {
            state.failures = 0;
        }
        is_known
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn url_token(url: &str) -> Option<String> {
    let url = Url::parse(url.trim()).ok()?;
    super::token(url.path()).map(str::to_owned)
}
