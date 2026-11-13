use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, SystemTime};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, sleep_until, timeout, timeout_at};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use vrcr_shared::console::{Component, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::sanitize_for_console;
use vrcr_shared::wire::{
    self, ClientHelloFrame, ResolveRequest, ResolveResponse, WelcomeCachedFrame, WelcomeFrame,
    action, fallback, feature, feedback, format,
};

use super::connect::{self, Probe, Socket};
use super::frames::{self, BinaryFrame};
use super::identity;
use super::welcome_cache::{self, WelcomeCache};
use crate::ipc::framing::cid_suffix;
use crate::ipc::{Feedback, Mesh, MeshResult, caller_opted_into_v2};
use crate::stats::Stats;

pub const PROXY_HOST: &str = "vrcresolver.com";
pub const RECONNECT_CAPS_SEC: [u64; 6] = [1, 2, 4, 8, 16, 30];
const MAX_RECENT_CIDS: usize = 256;
const RECENT_CIDS_TTL: Duration = Duration::from_secs(3600);
const MAX_RATE_LIMIT_COOLDOWN_SECONDS: i32 = 60;
const MAX_IDS_IN_LINE: usize = 8;
const PARSE_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

type UrlFor = Arc<dyn Fn(&str) -> String + Send + Sync>;
type Backoff = Arc<dyn Fn(u32) -> u64 + Send + Sync>;

pub fn mesh_url_for_host(host: &str) -> String {
    format!("wss://{}/mesh", host.trim())
}

pub fn should_send_client_hello(negotiated: Option<&str>) -> bool {
    negotiated == Some(wire::SUBPROTOCOL_V3)
}

fn random_u64() -> u64 {
    let bits = uuid::Uuid::new_v4().as_u128();
    u64::try_from(bits & u128::from(u64::MAX >> 2)).unwrap_or(0)
}

pub fn draw_backoff_ms(attempt: u32) -> u64 {
    let index = usize::try_from(attempt.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(RECONNECT_CAPS_SEC.len() - 1);
    let cap_ms = RECONNECT_CAPS_SEC[index] * 1000;
    random_u64() % (cap_ms + 1)
}

#[derive(Clone)]
pub struct Config {
    pub proxy_host: String,
    pub url_for_host: UrlFor,
    pub user_agent: String,
    pub client_id_path: PathBuf,
    pub welcome_cache_path: PathBuf,
    pub heartbeat: Duration,
    pub pong_deadline: Duration,
    pub send_deadline: Duration,
    pub welcome_timeout: Duration,
    pub stall_probe_after: Duration,
    pub stall_pong_deadline: Duration,
    pub connect_deadline: Duration,
    pub reconnect_grace: Duration,
    pub stable_link_after: Duration,
    pub min_wake_spacing: Duration,
    pub next_backoff_ms: Backoff,
}

impl Config {
    pub fn new(state_root: &Path, version: &str) -> Config {
        Config {
            proxy_host: PROXY_HOST.to_owned(),
            url_for_host: Arc::new(mesh_url_for_host),
            user_agent: format!("VRCResolver-Watchdog/{version}"),
            client_id_path: state_root.join(identity::FILE_NAME),
            welcome_cache_path: state_root.join(welcome_cache::FILE_NAME),
            heartbeat: Duration::from_secs(45),
            pong_deadline: Duration::from_secs(10),
            send_deadline: Duration::from_secs(10),
            welcome_timeout: Duration::from_secs(1),
            stall_probe_after: Duration::from_secs(6),
            stall_pong_deadline: Duration::from_secs(4),
            connect_deadline: Duration::from_secs(8),
            reconnect_grace: Duration::from_secs(5),
            stable_link_after: Duration::from_secs(30),
            min_wake_spacing: Duration::from_secs(1),
            next_backoff_ms: Arc::new(draw_backoff_ms),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn secs(d: Duration) -> String {
    format!("{}", d.as_secs_f64())
}

fn has_feature(features: Option<&[String]>, name: &str) -> bool {
    features.is_some_and(|f| f.iter().any(|x| x == name))
}

struct Link {
    sink: AsyncMutex<SplitSink<Socket, Message>>,
    open: AtomicBool,
    abort: watch::Sender<bool>,
    down: watch::Sender<bool>,
    welcome: watch::Sender<bool>,
    was_ready: AtomicBool,
    probing: AtomicBool,
    last_probe: Mutex<Option<Instant>>,
}

impl Link {
    fn new(sink: SplitSink<Socket, Message>) -> Link {
        Link {
            sink: AsyncMutex::new(sink),
            open: AtomicBool::new(true),
            abort: watch::channel(false).0,
            down: watch::channel(false).0,
            welcome: watch::channel(false).0,
            was_ready: AtomicBool::new(false),
            probing: AtomicBool::new(false),
            last_probe: Mutex::new(None),
        }
    }

    fn is_down(&self) -> bool {
        *self.down.borrow()
    }

    fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst) && !self.is_down()
    }

    fn abort(&self) {
        self.open.store(false, Ordering::SeqCst);
        self.abort.send_replace(true);
    }

    fn settle_welcome(&self) -> bool {
        self.welcome
            .send_if_modified(|w| !std::mem::replace(w, true))
    }

    async fn wait_down(&self) {
        let mut rx = self.down.subscribe();
        let _ = rx.wait_for(|d| *d).await;
    }
}

#[derive(Default)]
struct Server {
    protocol_version: i32,
    node: Option<String>,
    features: Option<Vec<String>>,
    warp_active: Option<bool>,
    server_version: Option<String>,
    yt_dlp_version: Option<String>,
    is_v3: bool,
    negotiated_format: String,
    is_msgpack: bool,
    current_host: String,
}

enum Attempt {
    Cancelled,
    Ended(Option<Arc<Link>>),
}

enum LinkWait {
    Ready(Arc<Link>),
    Reply(MeshResult),
    Gone,
}

enum ReplyWait {
    Got(MeshResult),
    Dropped,
}

struct Inner {
    cfg: Config,
    ux: Ux,
    log: Log,
    stats: Arc<Stats>,
    client_id: String,
    welcome_cache: WelcomeCache,
    tls: OnceLock<Result<Arc<rustls::ClientConfig>, String>>,
    first_party: Arc<RwLock<FirstPartyPolicy>>,
    server: Mutex<Server>,
    pending: Mutex<HashMap<String, oneshot::Sender<MeshResult>>>,
    inflight_cids: Mutex<HashMap<String, String>>,
    recent_cids: Mutex<HashMap<String, (String, Instant)>>,
    parse_failures: Mutex<HashMap<String, (Instant, u32)>>,
    current: Mutex<Option<Arc<Link>>>,
    ready: Mutex<Option<Arc<Link>>>,
    link_ready: watch::Sender<u64>,
    connect_settled: watch::Sender<u64>,
    wake: watch::Sender<bool>,
    shutdown: watch::Sender<bool>,
    connect_in_flight: AtomicBool,
    link_down_since: Mutex<Instant>,
    link_ready_at: Mutex<Option<Instant>>,
    last_attempt_start: Mutex<Instant>,
    last_pong: Mutex<Option<Instant>>,
    last_inbound: Mutex<Option<Instant>>,
    reconnect_attempt: AtomicU32,
    was_connected: AtomicBool,
    console_outage: Mutex<Option<Instant>>,
    rate_limited_until: Mutex<Option<Instant>>,
    last_probe: Mutex<Probe>,
    runner: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct MeshClient {
    inner: Arc<Inner>,
}

impl MeshClient {
    pub fn new(cfg: Config, ux: Ux, log: Log, stats: Arc<Stats>) -> MeshClient {
        let client_id = identity::load_or_create(&cfg.client_id_path);
        let welcome_cache = WelcomeCache::new(cfg.welcome_cache_path.clone(), log.clone());
        let now = Instant::now();
        MeshClient {
            inner: Arc::new(Inner {
                cfg,
                ux,
                log,
                stats,
                client_id,
                welcome_cache,
                tls: OnceLock::new(),
                first_party: Arc::new(RwLock::new(FirstPartyPolicy::default())),
                server: Mutex::new(Server {
                    negotiated_format: format::JSON.to_owned(),
                    ..Server::default()
                }),
                pending: Mutex::new(HashMap::new()),
                inflight_cids: Mutex::new(HashMap::new()),
                recent_cids: Mutex::new(HashMap::new()),
                parse_failures: Mutex::new(HashMap::new()),
                current: Mutex::new(None),
                ready: Mutex::new(None),
                link_ready: watch::channel(0).0,
                connect_settled: watch::channel(0).0,
                wake: watch::channel(false).0,
                shutdown: watch::channel(false).0,
                connect_in_flight: AtomicBool::new(false),
                link_down_since: Mutex::new(now),
                link_ready_at: Mutex::new(None),
                last_attempt_start: Mutex::new(now),
                last_pong: Mutex::new(None),
                last_inbound: Mutex::new(None),
                reconnect_attempt: AtomicU32::new(0),
                was_connected: AtomicBool::new(false),
                console_outage: Mutex::new(None),
                rate_limited_until: Mutex::new(None),
                last_probe: Mutex::new(Probe::default()),
                runner: Mutex::new(None),
            }),
        }
    }

    pub fn start(&self) {
        let handle = tokio::spawn(Arc::clone(&self.inner).run());
        *lock(&self.inner.runner) = Some(handle);
    }

    pub async fn stop(&self) {
        let inner = &self.inner;
        let link = lock(&inner.current).clone();
        inner.shutdown.send_replace(true);
        inner.fail_all_pending(fallback::SERVER_UNREACHABLE);
        if let Some(link) = link.filter(|l| l.is_open()) {
            let close = async {
                let mut sink = link.sink.lock().await;
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::Normal,
                        reason: "shutting down".into(),
                    })))
                    .await;
                drop(sink);
            };
            let _ = timeout(CLOSE_TIMEOUT, close).await;
        }
        let runner = lock(&inner.runner).take();
        if let Some(h) = runner {
            let _ = h.await;
        }
    }

    pub fn drop_connection(&self) {
        if let Some(link) = lock(&self.inner.current).clone() {
            link.abort();
        }
    }

    pub fn first_party(&self) -> Arc<RwLock<FirstPartyPolicy>> {
        Arc::clone(&self.inner.first_party)
    }

    pub fn client_id(&self) -> &str {
        &self.inner.client_id
    }

    pub fn server_protocol_version(&self) -> i32 {
        lock(&self.inner.server).protocol_version
    }

    pub fn server_node(&self) -> Option<String> {
        lock(&self.inner.server).node.clone()
    }

    pub fn server_version(&self) -> Option<String> {
        lock(&self.inner.server).server_version.clone()
    }

    pub fn yt_dlp_version(&self) -> Option<String> {
        lock(&self.inner.server).yt_dlp_version.clone()
    }

    pub fn warp_active(&self) -> Option<bool> {
        lock(&self.inner.server).warp_active
    }

    pub fn negotiated_format(&self) -> String {
        lock(&self.inner.server).negotiated_format.clone()
    }

    pub fn last_connect_probe(&self) -> Probe {
        *lock(&self.inner.last_probe)
    }

    pub fn reconnect_attempt(&self) -> u32 {
        self.inner.reconnect_attempt.load(Ordering::SeqCst)
    }
}

impl Mesh for MeshClient {
    fn current_node_host(&self) -> String {
        lock(&self.inner.server).current_host.clone()
    }

    fn is_connected(&self) -> bool {
        lock(&self.inner.current)
            .as_ref()
            .is_some_and(|l| l.is_open())
    }

    fn resolve(
        &self,
        req: &ResolveRequest,
        deadline: Instant,
    ) -> impl std::future::Future<Output = Result<MeshResult, String>> + Send {
        let inner = Arc::clone(&self.inner);
        let req = req.clone();
        async move { Ok(inner.resolve(req, deadline).await) }
    }

    fn send_playback_feedback(&self, fb: Feedback) {
        let inner = &self.inner;
        let Some(link) = lock(&inner.current).clone().filter(|l| l.is_open()) else {
            return;
        };
        if fb.url.is_empty() || fb.kind.is_empty() {
            return;
        }
        let features = lock(&inner.server).features.clone();
        let features = features.as_deref();
        if !has_feature(features, feature::PLAYBACK_FEEDBACK) {
            return;
        }
        let v2 = has_feature(features, feature::PLAYBACK_FEEDBACK_V2);
        let v2_kind = matches!(
            fb.kind,
            feedback::RESOLVED_REJECTED | feedback::OG_FAILED | feedback::CACHE_PLAY
        );
        if v2_kind && !v2 {
            return;
        }
        let detail = if v2 { fb.detail.as_deref() } else { None };
        let cid = fb
            .correlation_id
            .clone()
            .or_else(|| inner.lookup_recent_cid(&fb.url));
        let context =
            matches!(fb.kind, feedback::LOAD_FAILURE | feedback::SILENT_STALL).then(|| {
                inner.stats.playback.context(&fb.url).unwrap_or_else(|| {
                    crate::diagnostics::FailureContext {
                        source: "unknown".to_owned(),
                        ..Default::default()
                    }
                })
            });
        let payload = frames::playback_feedback_payload(crate::mesh::frames::PlaybackFeedback {
            url: &fb.url,
            kind: fb.kind,
            ms_since_open: fb.ms_since_open,
            client_id: &inner.client_id,
            correlation_id: cid.as_deref(),
            timestamp: SystemTime::now(),
            delivered_height: fb.delivered_height,
            detail,
            context: context.as_ref(),
        });
        let inner = Arc::clone(inner);
        tokio::spawn(async move {
            let _ = inner.send_text(&link, payload).await;
        });
    }
}

struct PendingGuard<'a> {
    inner: &'a Inner,
    id: String,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.inner.forget_pending(&self.id);
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Inner {
    fn tls_config(&self) -> Result<Arc<rustls::ClientConfig>, String> {
        self.tls.get_or_init(connect::platform_tls).clone()
    }

    fn is_msgpack(&self) -> bool {
        lock(&self.server).is_msgpack
    }

    fn now() -> Instant {
        Instant::now()
    }

    fn forget_pending(&self, id: &str) {
        lock(&self.pending).remove(id);
        lock(&self.inflight_cids).remove(id);
    }

    fn ready_link(&self) -> Option<Arc<Link>> {
        lock(&self.ready).clone().filter(|l| !l.is_down())
    }
}

fn json_frame(resp: &ResolveResponse) -> Vec<u8> {
    serde_json::to_vec(resp).unwrap_or_default()
}

#[cfg(test)]
#[path = "../client_tests/mod.rs"]
mod tests;

mod connection;

mod resolution;

mod protocol;
