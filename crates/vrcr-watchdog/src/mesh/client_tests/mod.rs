use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::Instant;

use vrcr_shared::console::{Style, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::wire::{ResolveRequest, ResolveResponse, WelcomeFrame, action, fallback, feature};

use super::{Config, MeshClient, PROXY_HOST, RECENT_CIDS_TTL};
use crate::ipc::{Mesh, MeshResult};
use crate::mesh::welcome_cache::WelcomeCache;
use crate::stats::Stats;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

struct Harness {
    client: MeshClient,
    out: Capture,
    log_path: std::path::PathBuf,
    dir: TempDir,
}

impl Harness {
    fn new() -> Harness {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let out = Capture::default();
        let ux = Ux::new(Box::new(out.clone()), Style::plain(), String::new);
        let log_path = dir.path().join("watchdog.log");
        let cfg = Config::new(dir.path(), "test");
        let client = MeshClient::new(
            cfg,
            ux,
            Log::to_file(&log_path),
            Arc::new(Stats::new(SystemTime::now())),
        );
        Harness {
            client,
            out,
            log_path,
            dir,
        }
    }

    fn v3(cache_has_node: bool) -> Harness {
        let h = Harness::new();
        if cache_has_node {
            seed_cache(&h.client.inner.cfg.welcome_cache_path);
        }
        {
            let mut s = super::lock(&h.client.inner.server);
            s.is_v3 = true;
            s.current_host = PROXY_HOST.to_owned();
        }
        h
    }

    async fn json(&self, text: &str) {
        self.client.inner.dispatch_text(None, text.as_bytes()).await;
    }

    async fn binary(&self, payload: &[u8]) {
        self.client.inner.dispatch_binary(None, payload).await;
    }

    fn pending(&self, id: &str) -> oneshot::Receiver<MeshResult> {
        let (tx, rx) = oneshot::channel();
        super::lock(&self.client.inner.pending).insert(id.to_owned(), tx);
        rx
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    fn cooldown(&self) -> Option<Instant> {
        *super::lock(&self.client.inner.rate_limited_until)
    }
}

fn seed_cache(path: &Path) {
    let cache = WelcomeCache::new(path.to_path_buf(), Log::disabled());
    let welcome = WelcomeFrame {
        protocol_version: 3,
        node: Some("node1".to_owned()),
        features: Some(vec![
            feature::PLAYBACK_FEEDBACK.to_owned(),
            feature::WELCOME_HOSTS.to_owned(),
        ]),
        first_party_hosts: Some(vec!["example.org".to_owned()]),
        server_version: Some("s1".to_owned()),
        ..WelcomeFrame::default()
    };
    cache.store(PROXY_HOST, &welcome, "hash1");
}

fn mp_frame(items: Vec<rmpv::Value>) -> Vec<u8> {
    let mut out = Vec::new();
    rmpv::encode::write_value(&mut out, &rmpv::Value::Array(items))
        .unwrap_or_else(|e| panic!("{e}"));
    out
}

fn mp_str(s: &str) -> rmpv::Value {
    rmpv::Value::from(s)
}

fn done(rx: &mut oneshot::Receiver<MeshResult>) -> Option<MeshResult> {
    rx.try_recv().ok()
}

mod contracts;

mod protocol;

mod resolution;

mod welcome;
