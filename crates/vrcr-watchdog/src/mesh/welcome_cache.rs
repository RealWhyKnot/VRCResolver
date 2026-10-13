use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use vrcr_shared::log::Log;
use vrcr_shared::logutil::{UtcStamp, iso8601_utc};
use vrcr_shared::wire::WelcomeFrame;

use super::identity::sibling;

pub const FILE_NAME: &str = "v3_welcome_cache.json";
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub welcome_hash: Option<String>,
    pub protocol_version: i32,
    pub node: Option<String>,
    pub engines: Option<Vec<String>>,
    pub features: Option<Vec<String>>,
    pub warp_active: Option<bool>,
    pub yt_dlp_version: Option<String>,
    pub server_version: Option<String>,
    pub first_party_hosts: Option<Vec<String>>,
    pub playback_proxy_paths: Option<Vec<String>>,
    pub stored_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct CacheFile {
    nodes: Option<BTreeMap<String, Entry>>,
}

pub struct WelcomeCache {
    path: PathBuf,
    log: Log,
}

impl WelcomeCache {
    pub fn new(path: PathBuf, log: Log) -> WelcomeCache {
        WelcomeCache { path, log }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, node: &str) -> Option<Entry> {
        if node.is_empty() {
            return None;
        }
        let nodes = self.load_file()?.nodes?;
        find_key(&nodes, node).and_then(|k| nodes.get(&k).cloned())
    }

    pub fn store(&self, node: &str, welcome: &WelcomeFrame, hash: &str) {
        if node.is_empty() || hash.is_empty() {
            return;
        }
        let mut file = self.load_file().unwrap_or_default();
        let nodes = file.nodes.get_or_insert_with(BTreeMap::new);
        if let Some(existing) = find_key(nodes, node) {
            nodes.remove(&existing);
        }
        nodes.insert(
            node.to_owned(),
            Entry {
                welcome_hash: Some(hash.to_owned()),
                protocol_version: welcome.protocol_version,
                node: welcome.node.clone(),
                engines: welcome.engines.clone(),
                features: welcome.features.clone(),
                warp_active: welcome.warp_active,
                yt_dlp_version: welcome.yt_dlp_version.clone(),
                server_version: welcome.server_version.clone(),
                first_party_hosts: welcome.first_party_hosts.clone(),
                playback_proxy_paths: welcome.playback_proxy_paths.clone(),
                stored_at: Some(iso8601_utc(SystemTime::now())),
            },
        );
        self.save_file(&file);
    }

    pub fn invalidate(&self, node: &str) {
        if node.is_empty() {
            return;
        }
        let Some(mut file) = self.load_file() else {
            return;
        };
        let Some(nodes) = file.nodes.as_mut() else {
            return;
        };
        let Some(key) = find_key(nodes, node) else {
            return;
        };
        nodes.remove(&key);
        self.save_file(&file);
    }

    fn load_file(&self) -> Option<CacheFile> {
        let meta = fs::metadata(&self.path).ok()?;
        if meta.len() > MAX_FILE_BYTES {
            self.log.line(&format!(
                "[v3-cache] oversized cache file at {} ({} bytes; cap {MAX_FILE_BYTES}) -- renaming aside, treating as cache miss",
                self.path.display(),
                meta.len()
            ));
            let s = UtcStamp::at(SystemTime::now());
            let aside = sibling(
                &self.path,
                &format!(
                    ".oversized-{:04}{:02}{:02}{:02}{:02}{:02}",
                    s.year, s.month, s.day, s.hour, s.minute, s.second
                ),
            );
            let _ = fs::rename(&self.path, aside);
            return None;
        }
        let bytes = fs::read(&self.path).ok()?;
        serde_json::from_slice::<Option<CacheFile>>(&bytes).ok().flatten()
    }

    fn save_file(&self, file: &CacheFile) {
        let tmp = sibling(&self.path, ".new");
        let written = self
            .path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| serde_json::to_vec(file).map_err(std::io::Error::other))
            .and_then(|bytes| fs::write(&tmp, bytes))
            .and_then(|()| fs::rename(&tmp, &self.path));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

fn find_key(nodes: &BTreeMap<String, Entry>, node: &str) -> Option<String> {
    if nodes.contains_key(node) {
        return Some(node.to_owned());
    }
    nodes.keys().find(|k| k.eq_ignore_ascii_case(node)).cloned()
}
