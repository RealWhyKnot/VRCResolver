use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vrcr_shared::log::Log;
use vrcr_shared::logutil::{self, UtcStamp};
use vrcr_shared::wire::{ResolveResponse, action};

use crate::clock::Now;

pub const MAX_ENTRIES: usize = 500;
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub const FLUSH_DEBOUNCE: Duration = Duration::from_secs(5);
const EXPIRY_SAFETY_MARGIN: Duration = Duration::from_secs(30);
const DEFAULT_EXPIRY_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct CachedResolve {
    pub frame: Vec<u8>,
    pub action: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default = "one")]
    version: i32,
    #[serde(default, deserialize_with = "null_map")]
    entries: BTreeMap<String, Entry>,
}

fn one() -> i32 {
    1
}

fn null_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, Entry>, D::Error> {
    Ok(Option::deserialize(d)?.unwrap_or_default())
}

impl Default for CacheFile {
    fn default() -> Self {
        CacheFile {
            version: 1,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Entry {
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    player: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<ResolveResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fetched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
}

pub struct Key<'a> {
    pub node: &'a str,
    pub url: &'a str,
    pub player: Option<&'a str>,
    pub format_arg: Option<&'a str>,
    pub max_height: Option<i32>,
}

impl Key<'_> {
    pub fn hash(&self) -> String {
        let text = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.node,
            self.url,
            self.player.unwrap_or_default(),
            self.max_height.map(|h| h.to_string()).unwrap_or_default(),
            self.format_arg.unwrap_or_default()
        );
        let digest = Sha256::digest(text.as_bytes());
        hex::encode(&digest[..16])
    }

    fn usable(&self) -> bool {
        !self.node.is_empty() && !self.url.is_empty()
    }
}

struct Inner {
    file: CacheFile,
    loaded: bool,
    dirty: bool,
    due: Option<Instant>,
    flusher: bool,
}

struct Shared {
    path: PathBuf,
    now: Now,
    debounce: Duration,
    log: Log,
    inner: Mutex<Inner>,
    wake: Condvar,
}

pub struct ResolveCache {
    shared: Arc<Shared>,
}

impl ResolveCache {
    pub fn new(path: PathBuf, now: Now, log: Log) -> ResolveCache {
        ResolveCache::with_debounce(path, now, log, FLUSH_DEBOUNCE)
    }

    pub fn with_debounce(path: PathBuf, now: Now, log: Log, debounce: Duration) -> ResolveCache {
        ResolveCache {
            shared: Arc::new(Shared {
                path,
                now,
                debounce,
                log,
                inner: Mutex::new(Inner {
                    file: CacheFile::default(),
                    loaded: false,
                    dirty: false,
                    due: None,
                    flusher: false,
                }),
                wake: Condvar::new(),
            }),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.shared.path
    }

    pub fn lookup(&self, key: &Key<'_>, request_id: &str) -> Option<CachedResolve> {
        if !key.usable() {
            return None;
        }
        let hash = key.hash();
        let now = (self.shared.now)();
        let mut g = self.shared.loaded();
        let entry = g.file.entries.get(&hash)?;
        if !is_live(entry.expires_at.as_deref(), now) {
            g.file.entries.remove(&hash);
            g.dirty = true;
            self.schedule(&mut g);
            return None;
        }
        let mut copy = entry.response.clone()?;
        drop(g);
        copy.id = request_id.to_owned();
        let frame = serde_json::to_vec(&copy).ok()?;
        let action = if copy.action.is_empty() {
            action::RESOLVED.to_owned()
        } else {
            copy.action
        };
        Some(CachedResolve {
            frame,
            action,
            reason: copy.reason,
        })
    }

    pub fn store(&self, key: &Key<'_>, response: &ResolveResponse) -> Option<String> {
        if response.action != action::RESOLVED || !key.usable() {
            return None;
        }
        let now = (self.shared.now)();
        let expires_at = match response.expires_at.as_deref() {
            Some(e) if !e.is_empty() => e.to_owned(),
            _ => logutil::iso8601_utc(now + DEFAULT_EXPIRY_TTL),
        };
        let hash = key.hash();
        let mut stored = response.clone();
        stored.expires_at = Some(expires_at.clone());
        let entry = Entry {
            key: Some(hash.clone()),
            node: Some(key.node.to_owned()),
            url: Some(key.url.to_owned()),
            player: key.player.map(str::to_owned),
            format_arg: key.format_arg.map(str::to_owned),
            response: Some(stored),
            fetched_at: Some(logutil::iso8601_utc(now)),
            expires_at: Some(expires_at.clone()),
        };
        let mut g = self.shared.loaded();
        g.file.entries.insert(hash, entry);
        evict_past_cap(&mut g.file.entries);
        g.dirty = true;
        self.schedule(&mut g);
        Some(expires_at)
    }

    pub fn evict_by_url(&self, url: &str) -> usize {
        if url.is_empty() {
            return 0;
        }
        let mut g = self.shared.loaded();
        let before = g.file.entries.len();
        g.file.entries.retain(|_, e| {
            e.url.as_deref() != Some(url) && e.response.as_ref().and_then(|r| r.url.as_deref()) != Some(url)
        });
        let removed = before - g.file.entries.len();
        if removed > 0 {
            g.dirty = true;
            self.schedule(&mut g);
        }
        removed
    }

    pub fn source_url_for_resolved(&self, resolved_url: &str) -> Option<String> {
        if resolved_url.is_empty() {
            return None;
        }
        let g = self.shared.loaded();
        g.file
            .entries
            .values()
            .find(|e| e.response.as_ref().and_then(|r| r.url.as_deref()) == Some(resolved_url))
            .and_then(|e| e.url.clone())
            .filter(|u| !u.is_empty())
    }

    pub fn count(&self) -> usize {
        self.shared.loaded().file.entries.len()
    }

    pub fn flush_now(&self) {
        self.shared.flush_now();
    }

    fn schedule(&self, g: &mut MutexGuard<'_, Inner>) {
        g.due = Some(Instant::now() + self.shared.debounce);
        if g.flusher {
            self.shared.wake.notify_all();
            return;
        }
        let shared = self.shared.clone();
        g.flusher = std::thread::Builder::new()
            .name("resolve-cache-flush".to_owned())
            .spawn(move || shared.flush_loop())
            .is_ok();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn loaded(&self) -> MutexGuard<'_, Inner> {
        let mut g = self.lock();
        if !g.loaded {
            let mut file = self.load_file().unwrap_or_default();
            let now = (self.now)();
            let before = file.entries.len();
            file.entries.retain(|_, e| is_live(e.expires_at.as_deref(), now));
            g.dirty |= file.entries.len() != before;
            g.file = file;
            g.loaded = true;
        }
        g
    }

    fn flush_loop(&self) {
        let mut g = self.lock();
        loop {
            match g.due {
                None => {
                    g.flusher = false;
                    return;
                }
                Some(due) => {
                    let now = Instant::now();
                    if now < due {
                        g = self
                            .wake
                            .wait_timeout(g, due - now)
                            .map(|(g, _)| g)
                            .unwrap_or_else(|e| e.into_inner().0);
                        continue;
                    }
                    g.due = None;
                    drop(g);
                    self.flush_now();
                    g = self.lock();
                }
            }
        }
    }

    fn flush_now(&self) {
        let snapshot = {
            let mut g = self.lock();
            if !g.dirty {
                return;
            }
            g.dirty = false;
            g.file.clone()
        };
        self.save_file(&snapshot);
    }

    fn load_file(&self) -> Option<CacheFile> {
        let meta = fs::metadata(&self.path).ok()?;
        if meta.len() > MAX_FILE_BYTES {
            self.log.line(&format!(
                "[resolve-cache] oversized cache file at {} ({} bytes; cap {MAX_FILE_BYTES}) -- renaming aside, treating as cache miss",
                self.path.display(),
                meta.len()
            ));
            let s = UtcStamp::at((self.now)());
            let mut aside = self.path.as_os_str().to_owned();
            aside.push(format!(
                ".oversized-{:04}{:02}{:02}{:02}{:02}{:02}",
                s.year, s.month, s.day, s.hour, s.minute, s.second
            ));
            let _ = fs::rename(&self.path, PathBuf::from(aside));
            return None;
        }
        let bytes = fs::read(&self.path).ok()?;
        serde_json::from_slice::<Option<CacheFile>>(&bytes).ok().flatten()
    }

    fn save_file(&self, file: &CacheFile) {
        let mut tmp = self.path.as_os_str().to_owned();
        tmp.push(".new");
        let tmp = PathBuf::from(tmp);
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
                fs::create_dir_all(dir)?;
            }
            fs::write(&tmp, serde_json::to_vec(file).map_err(std::io::Error::other)?)?;
            fs::rename(&tmp, &self.path)
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            self.log
                .line(&format!("[resolve-cache] flush failed: {:?}", e.kind()));
        }
    }
}

fn is_live(expires_at: Option<&str>, now: SystemTime) -> bool {
    expires_at
        .and_then(logutil::parse_iso8601_utc)
        .is_some_and(|t| t > now + EXPIRY_SAFETY_MARGIN)
}

fn evict_past_cap(entries: &mut BTreeMap<String, Entry>) {
    if entries.len() <= MAX_ENTRIES {
        return;
    }
    let mut by_age: Vec<(String, String)> = entries
        .iter()
        .map(|(k, e)| (e.fetched_at.clone().unwrap_or_default(), k.clone()))
        .collect();
    by_age.sort();
    let excess = entries.len() - MAX_ENTRIES;
    for (_, k) in by_age.into_iter().take(excess) {
        entries.remove(&k);
    }
}
