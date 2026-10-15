use std::collections::HashMap;
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
use vrcr_shared::logutil::{payload_preview, sanitize_for_console};
use vrcr_shared::wire::{
    self, ClientHelloFrame, ResolveRequest, ResolveResponse, WelcomeCachedFrame, WelcomeFrame, action,
    fallback, feature, feedback, format,
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
        self.welcome.send_if_modified(|w| !std::mem::replace(w, true))
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
        lock(&self.inner.current).as_ref().is_some_and(|l| l.is_open())
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
        let payload = frames::playback_feedback_payload(
            &fb.url,
            fb.kind,
            fb.ms_since_open,
            &inner.client_id,
            cid.as_deref(),
            SystemTime::now(),
            fb.delivered_height,
            detail,
        );
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

    async fn run(self: Arc<Self>) {
        let mut shutdown = self.shutdown.subscribe();
        loop {
            if *shutdown.borrow() {
                break;
            }
            *lock(&self.last_attempt_start) = Self::now();
            let link = match self.attempt(&mut shutdown).await {
                Attempt::Cancelled => break,
                Attempt::Ended(link) => link,
            };
            if *shutdown.borrow() {
                break;
            }
            let stayed_up = link.as_ref().is_some_and(|l| l.was_ready.load(Ordering::SeqCst))
                && lock(&self.link_ready_at).is_some_and(|t| t.elapsed() >= self.cfg.stable_link_after);
            if stayed_up {
                self.reconnect_attempt.store(0, Ordering::SeqCst);
            }
            let attempt = self.reconnect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
            self.stats.record_reconnect();
            let wait_ms = (self.cfg.next_backoff_ms)(attempt);
            if attempt <= 5 || attempt.is_multiple_of(10) {
                self.ux.write(
                    Component::Mesh,
                    &format!("reconnect attempt {attempt} in {} s", wait_ms / 1000),
                );
            }
            if !self.wait_before_reconnect(wait_ms, &mut shutdown).await {
                break;
            }
        }
    }

    async fn wait_before_reconnect(&self, wait_ms: u64, shutdown: &mut watch::Receiver<bool>) -> bool {
        let wait = Duration::from_millis(wait_ms);
        let backoff_end = Self::now() + wait;
        let mut wake = self.wake.subscribe();
        let woke = tokio::select! {
            () = sleep(wait) => false,
            _ = wake.wait_for(|w| *w) => true,
            _ = shutdown.wait_for(|s| *s) => return false,
        };
        if woke {
            let wake_at = (*lock(&self.last_attempt_start) + self.cfg.min_wake_spacing).min(backoff_end);
            tokio::select! {
                () = sleep_until(wake_at) => {}
                _ = shutdown.wait_for(|s| *s) => return false,
            }
        }
        if *shutdown.borrow() {
            return false;
        }
        self.wake.send_replace(false);
        true
    }

    fn wake_reconnect_loop(&self) {
        self.wake.send_replace(true);
    }

    fn settle_connect(&self) {
        self.connect_in_flight.store(false, Ordering::SeqCst);
        self.connect_settled.send_modify(|g| *g = g.wrapping_add(1));
    }

    fn prepare_welcome_state(&self, node: &str) {
        *lock(&self.server) = Server {
            negotiated_format: format::JSON.to_owned(),
            current_host: node.to_owned(),
            ..Server::default()
        };
    }

    fn report_failure(&self, text: &str) {
        let attempt = self.reconnect_attempt.load(Ordering::SeqCst);
        if self.was_connected.load(Ordering::SeqCst) || attempt == 0 {
            self.ux.warn(Component::Mesh, text);
        } else {
            self.log
                .line(&format!("[mesh] reconnect attempt {attempt} failed: {text}"));
        }
        self.was_connected.store(false, Ordering::SeqCst);
    }

    async fn attempt(self: &Arc<Self>, shutdown: &mut watch::Receiver<bool>) -> Attempt {
        let node = self.cfg.proxy_host.clone();
        self.prepare_welcome_state(&node);
        let url = (self.cfg.url_for_host)(&node);
        self.connect_in_flight.store(true, Ordering::SeqCst);
        let mut probe = Probe::default();
        let started = Self::now();
        let outcome = tokio::select! {
            r = timeout(
                self.cfg.connect_deadline,
                connect::open(&url, &self.cfg.user_agent, || self.tls_config(), &mut probe),
            ) => Some(r),
            _ = shutdown.wait_for(|s| *s) => None,
        };
        *lock(&self.last_probe) = probe;
        let elapsed_ms = started.elapsed().as_millis();
        let opened = match outcome {
            None => {
                self.settle_connect();
                return Attempt::Cancelled;
            }
            Some(Ok(Ok(opened))) => opened,
            Some(Ok(Err(e))) => {
                let text = frames::describe_connect_failure(&node, elapsed_ms, &probe, Some(&e.describe()));
                self.report_failure(&text);
                self.settle_connect();
                return Attempt::Ended(None);
            }
            Some(Err(_)) => {
                let text = frames::describe_connect_failure(&node, elapsed_ms, &probe, None);
                self.report_failure(&text);
                self.settle_connect();
                return Attempt::Ended(None);
            }
        };

        self.log
            .line(&format!("[mesh] connected in {elapsed_ms} ms{}", probe.timings()));
        let is_v3 = should_send_client_hello(opened.subprotocol.as_deref());
        lock(&self.server).is_v3 = is_v3;
        self.log.line(&format!(
            "[mesh][v3] negotiated subprotocol={} v3={is_v3} deflate-offered=false",
            opened.subprotocol.as_deref().unwrap_or("<none>")
        ));

        let (sink, stream) = opened.socket.split();
        let link = Arc::new(Link::new(sink));
        *lock(&self.current) = Some(Arc::clone(&link));
        if is_v3 && let Err(e) = self.send_client_hello(&link, &node).await {
            self.log.line(&format!(
                "[mesh][v3] client_hello send failed: {}",
                sanitize_for_console(&e, 160)
            ));
        }
        self.was_connected.store(true, Ordering::SeqCst);
        *lock(&self.last_pong) = Some(Self::now());
        self.arm_welcome(&link);
        self.ux.write(Component::Mesh, &format!("connected node={node}"));

        let pumped = tokio::select! {
            r = self.pump(&link, stream) => Some(r),
            _ = shutdown.wait_for(|s| *s) => None,
        };
        match &pumped {
            Some(Ok(())) => self.ux.write(Component::Mesh, "disconnected (clean close)"),
            Some(Err(e)) => {
                self.report_failure(&format!("disconnected (error): {}", sanitize_for_console(e, 160)))
            }
            None => {}
        }
        self.mark_link_down(&link);
        self.settle_connect();
        link.settle_welcome();
        link.open.store(false, Ordering::SeqCst);
        {
            let mut current = lock(&self.current);
            if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &link)) {
                *current = None;
            }
        }
        match pumped {
            None => Attempt::Cancelled,
            Some(_) => Attempt::Ended(Some(link)),
        }
    }

    async fn send_client_hello(&self, link: &Link, node: &str) -> Result<(), String> {
        let hash = self.welcome_cache.get(node).and_then(|e| e.welcome_hash);
        let hello = ClientHelloFrame {
            action: action::CLIENT_HELLO.to_owned(),
            welcome_hash: hash.clone(),
            client_id: self.client_id.clone(),
            accept_formats: Some(format::PREFERENCE.iter().map(|f| (*f).to_owned()).collect()),
            ..Default::default()
        };
        let text = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
        self.send_text(link, text).await?;
        self.log.line(&format!(
            "[mesh][v3] client_hello sent node={node} hash={}",
            hash.as_deref().unwrap_or("null")
        ));
        Ok(())
    }

    fn arm_welcome(self: &Arc<Self>, link: &Arc<Link>) {
        let inner = Arc::clone(self);
        let link = Arc::clone(link);
        let wait = self.cfg.welcome_timeout;
        tokio::spawn(async move {
            let mut rx = link.welcome.subscribe();
            let settled = tokio::select! {
                _ = rx.wait_for(|w| *w) => true,
                () = sleep(wait) => false,
            };
            if !settled && link.settle_welcome() {
                {
                    let mut s = lock(&inner.server);
                    if s.protocol_version == 0 {
                        s.protocol_version = 1;
                    }
                }
                inner
                    .log
                    .line("[mesh] no welcome within 1s -- assuming v1 server");
            }
            inner.mark_link_ready(&link);
        });
    }

    fn mark_link_ready(&self, link: &Arc<Link>) {
        if !link.is_open() {
            return;
        }
        *lock(&self.link_ready_at) = Some(Self::now());
        link.was_ready.store(true, Ordering::SeqCst);
        self.wake.send_replace(false);
        *lock(&self.ready) = Some(Arc::clone(link));
        self.link_ready.send_modify(|g| *g = g.wrapping_add(1));
        self.settle_connect();
    }

    fn mark_link_down(&self, link: &Arc<Link>) {
        {
            let mut ready = lock(&self.ready);
            if ready.as_ref().is_some_and(|r| Arc::ptr_eq(r, link)) {
                *ready = None;
            }
        }
        if link.was_ready.load(Ordering::SeqCst) {
            *lock(&self.link_down_since) = Self::now();
        }
        link.down.send_replace(true);
    }

    async fn send_text(&self, link: &Link, text: String) -> Result<bool, String> {
        if !link.is_open() {
            return Ok(false);
        }
        let deadline = Self::now() + self.cfg.send_deadline;
        let Ok(mut sink) = timeout_at(deadline, link.sink.lock()).await else {
            self.log.line(&format!(
                "[mesh] send skipped: another send has been stuck for over {}s",
                secs(self.cfg.send_deadline)
            ));
            return Ok(false);
        };
        if !link.is_open() {
            return Ok(false);
        }
        match timeout_at(deadline, sink.send(Message::text(text))).await {
            Ok(Ok(())) => Ok(true),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => {
                self.ux.warn(
                    Component::Mesh,
                    &format!(
                        "send stalled for {}s; dropping the connection to reconnect",
                        secs(self.cfg.send_deadline)
                    ),
                );
                link.abort();
                Ok(false)
            }
        }
    }

    async fn pump(self: &Arc<Self>, link: &Arc<Link>, mut stream: SplitStream<Socket>) -> Result<(), String> {
        let _heartbeat = AbortOnDrop(tokio::spawn(Arc::clone(self).heartbeat(Arc::clone(link))));
        let mut abort = link.abort.subscribe();
        loop {
            let next = tokio::select! {
                _ = abort.wait_for(|a| *a) => return Err("connection aborted".to_owned()),
                m = stream.next() => m,
            };
            let msg = match next {
                None => return Err("connection closed without a close frame".to_owned()),
                Some(Err(e)) => return Err(e.to_string()),
                Some(Ok(m)) => m,
            };
            *lock(&self.last_inbound) = Some(Self::now());
            match msg {
                Message::Text(t) => self.dispatch_text(Some(link), t.as_str().as_bytes()).await,
                Message::Binary(b) => {
                    if self.is_msgpack() {
                        self.dispatch_binary(Some(link), &b).await;
                    } else {
                        self.ux.warn(
                            Component::Mesh,
                            "unexpected Binary frame on json-negotiated connection -- aborting + reconnecting",
                        );
                        link.abort();
                        return Ok(());
                    }
                }
                Message::Close(_) => return Ok(()),
                _ => {}
            }
        }
    }

    async fn heartbeat(self: Arc<Self>, link: Arc<Link>) {
        loop {
            sleep(self.cfg.heartbeat).await;
            if !link.is_open() {
                return;
            }
            let sent_at = Self::now();
            if self
                .send_text(&link, frames::PING_FRAME.to_owned())
                .await
                .is_err()
            {
                link.abort();
                return;
            }
            sleep(self.cfg.pong_deadline).await;
            let last_pong = *lock(&self.last_pong);
            let Some(pong) = last_pong.filter(|p| *p >= sent_at) else {
                link.abort();
                return;
            };
            self.stats.perf.record_mesh_rtt(pong - sent_at);
        }
    }

    fn probe_if_quiet(self: &Arc<Self>, link: &Arc<Link>, quiet_since: Instant) {
        if lock(&self.last_inbound).is_some_and(|t| t >= quiet_since) || link.is_down() {
            return;
        }
        if link.probing.swap(true, Ordering::SeqCst) {
            return;
        }
        {
            let mut last = lock(&link.last_probe);
            if last.is_some_and(|t| t >= quiet_since) {
                link.probing.store(false, Ordering::SeqCst);
                return;
            }
            *last = Some(Self::now());
        }
        tokio::spawn(Arc::clone(self).probe_link(Arc::clone(link)));
    }

    async fn probe_link(self: Arc<Self>, link: Arc<Link>) {
        let probe_at = Self::now();
        match self.send_text(&link, frames::PING_FRAME.to_owned()).await {
            Err(_) => link.abort(),
            Ok(false) => {}
            Ok(true) => {
                tokio::select! {
                    () = sleep(self.cfg.stall_pong_deadline) => {}
                    () = link.wait_down() => {}
                }
                let answered = lock(&self.last_inbound).is_some_and(|t| t >= probe_at);
                if !link.is_down() && !answered {
                    self.log.line(&format!(
                        "[mesh] no answer to a ping within {}s while a resolve waited; dropping the connection to reconnect",
                        secs(self.cfg.stall_pong_deadline)
                    ));
                    link.abort();
                }
            }
        }
        link.probing.store(false, Ordering::SeqCst);
    }

    async fn wait_for_link(&self, reply: &mut oneshot::Receiver<MeshResult>) -> LinkWait {
        let mut woke = false;
        loop {
            let mut next = self.link_ready.subscribe();
            if let Some(link) = self.ready_link() {
                return LinkWait::Ready(link);
            }
            if !woke {
                self.wake_reconnect_loop();
                woke = true;
            }
            let mut settled = self.connect_settled.subscribe();
            let grace_end = *lock(&self.link_down_since) + self.cfg.reconnect_grace;
            let grace_ran_out = grace_end <= Self::now();
            if grace_ran_out && !self.connect_in_flight.load(Ordering::SeqCst) {
                return LinkWait::Gone;
            }
            tokio::select! {
                _ = next.changed() => {
                    woke = false;
                }
                r = &mut *reply => {
                    return match r {
                        Ok(m) => LinkWait::Reply(m),
                        Err(_) => LinkWait::Gone,
                    };
                }
                () = sleep_until(grace_end), if !grace_ran_out => {}
                _ = settled.changed(), if grace_ran_out => {
                    return match self.ready_link() {
                        Some(link) => LinkWait::Ready(link),
                        None => LinkWait::Gone,
                    };
                }
            }
        }
    }

    async fn wait_for_reply(
        self: &Arc<Self>,
        link: &Arc<Link>,
        reply: &mut oneshot::Receiver<MeshResult>,
    ) -> ReplyWait {
        loop {
            let window_start = Self::now();
            tokio::select! {
                r = &mut *reply => {
                    return match r {
                        Ok(m) => ReplyWait::Got(m),
                        Err(_) => ReplyWait::Dropped,
                    };
                }
                () = link.wait_down() => return ReplyWait::Dropped,
                () = sleep(self.cfg.stall_probe_after) => self.probe_if_quiet(link, window_start),
            }
        }
    }

    async fn resolve(self: Arc<Self>, mut req: ResolveRequest, deadline: Instant) -> MeshResult {
        if req.id.is_empty() {
            req.id = identity::fresh_id();
        }
        let id = req.id.clone();
        if lock(&self.rate_limited_until).is_some_and(|until| until > Self::now()) {
            return frames::fallback_result(&id, fallback::RATE_LIMITED);
        }
        let (tx, mut rx) = oneshot::channel();
        lock(&self.pending).insert(id.clone(), tx);
        let cid = req
            .correlation_id
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| id.clone());
        lock(&self.inflight_cids).insert(id.clone(), cid);
        let _guard = PendingGuard {
            inner: &self,
            id: id.clone(),
        };
        match timeout_at(deadline, self.resolve_loop(&mut req, &mut rx, deadline)).await {
            Ok(result) => result,
            Err(_) => frames::fallback_result(&id, fallback::CLIENT_DEADLINE_EXCEEDED),
        }
    }

    async fn resolve_loop(
        self: &Arc<Self>,
        req: &mut ResolveRequest,
        rx: &mut oneshot::Receiver<MeshResult>,
        deadline: Instant,
    ) -> MeshResult {
        let id = req.id.clone();
        let cid = cid_suffix(req.correlation_id.as_deref());
        let mut sends = 0u32;
        let mut drops = 0u32;
        loop {
            let link = match self.wait_for_link(rx).await {
                LinkWait::Reply(m) => return m,
                LinkWait::Gone => {
                    self.forget_pending(&id);
                    if sends > 0 {
                        self.log.line(&format!(
                            "[mesh] no reconnect within {}s; failing id={id}{cid}",
                            secs(self.cfg.reconnect_grace)
                        ));
                    }
                    return frames::fallback_result(&id, fallback::SERVER_UNREACHABLE);
                }
                LinkWait::Ready(link) => link,
            };
            if let Ok(m) = rx.try_recv() {
                return m;
            }
            if lock(&self.server).protocol_version >= 2
                && req.protocol_version.is_none()
                && caller_opted_into_v2(req)
            {
                req.protocol_version = Some(wire::CLIENT_PROTOCOL_VERSION);
            }
            req.wrapper_deadline_ms = Some(frames::wire_deadline_for(deadline, Self::now()));
            let payload = match serde_json::to_string(&*req) {
                Ok(p) => p,
                Err(e) => {
                    self.forget_pending(&id);
                    self.ux.warn(
                        Component::Mesh,
                        &format!("request serialization failed id={id}: {e}"),
                    );
                    return frames::fallback_result(&id, fallback::INTERNAL_ERROR);
                }
            };
            let sent = match self.send_text(&link, payload).await {
                Ok(s) => s,
                Err(e) => {
                    self.ux.warn(
                        Component::Mesh,
                        &format!("send failed id={id}{cid}: {}", sanitize_for_console(&e, 160)),
                    );
                    false
                }
            };
            if !sent {
                let link_dropped = tokio::select! {
                    () = link.wait_down() => true,
                    r = &mut *rx => {
                        return r.unwrap_or_else(|_| frames::fallback_result(&id, fallback::SERVER_UNREACHABLE));
                    }
                    () = sleep(self.cfg.reconnect_grace) => {
                        self.forget_pending(&id);
                        return frames::fallback_result(&id, fallback::SERVER_UNREACHABLE);
                    }
                };
                if link_dropped && sends > 0 {
                    drops += 1;
                    if drops > 1 {
                        return self.give_up_after_resend(&id, &cid);
                    }
                }
                continue;
            }
            sends += 1;
            if sends > 1 {
                self.log.line(&format!(
                    "[mesh] resent id={id}{cid} after reconnect (send {sends})"
                ));
            }
            match self.wait_for_reply(&link, rx).await {
                ReplyWait::Got(m) => return m,
                ReplyWait::Dropped => {
                    if let Ok(m) = rx.try_recv() {
                        return m;
                    }
                }
            }
            drops += 1;
            if drops > 1 {
                return self.give_up_after_resend(&id, &cid);
            }
            self.log.line(&format!(
                "[mesh] connection dropped with id={id}{cid} pending; resending after reconnect"
            ));
        }
    }

    fn give_up_after_resend(&self, id: &str, cid: &str) -> MeshResult {
        self.forget_pending(id);
        self.log.line(&format!(
            "[mesh] connection dropped again with id={id}{cid} pending; not resending a second time"
        ));
        frames::fallback_result(id, fallback::SERVER_UNREACHABLE)
    }

    fn fail_all_pending(&self, reason: &str) {
        lock(&self.inflight_cids).clear();
        let mut drained: Vec<(String, oneshot::Sender<MeshResult>)> = lock(&self.pending).drain().collect();
        if drained.is_empty() {
            return;
        }
        drained.sort_by(|a, b| a.0.cmp(&b.0));
        let ids: Vec<&str> = drained.iter().map(|(id, _)| id.as_str()).collect();
        let list = if ids.len() <= MAX_IDS_IN_LINE {
            ids.join(",")
        } else {
            format!(
                "{},...(+{})",
                ids[..MAX_IDS_IN_LINE].join(","),
                ids.len() - MAX_IDS_IN_LINE
            )
        };
        self.ux.warn(
            Component::Mesh,
            &format!(
                "failing {} pending requests reason={reason} ids={list}",
                drained.len()
            ),
        );
        for (id, tx) in drained {
            let _ = tx.send(frames::fallback_result(&id, reason));
        }
    }

    fn complete(&self, id: &str, result: MeshResult) -> bool {
        let Some(tx) = lock(&self.pending).remove(id) else {
            return false;
        };
        let _ = tx.send(result);
        true
    }

    fn lookup_recent_cid(&self, url: &str) -> Option<String> {
        let mut recent = lock(&self.recent_cids);
        let (cid, at) = recent.get(url)?.clone();
        if at.elapsed() > RECENT_CIDS_TTL {
            recent.remove(url);
            return None;
        }
        Some(cid)
    }

    fn remember_resolved_url_cid(&self, url: &str, cid: &str) {
        if url.is_empty() || cid.is_empty() {
            return;
        }
        let mut recent = lock(&self.recent_cids);
        recent.insert(url.to_owned(), (cid.to_owned(), Self::now()));
        if recent.len() <= MAX_RECENT_CIDS {
            return;
        }
        let oldest = recent
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            recent.remove(&k);
        }
    }

    fn parse_failure_count(&self, key: &str) -> Option<u32> {
        let now = Self::now();
        let mut map = lock(&self.parse_failures);
        match map.get_mut(key) {
            Some((last, count)) => {
                *count += 1;
                if now.duration_since(*last) >= PARSE_FAILURE_WINDOW {
                    *last = now;
                    Some(*count)
                } else {
                    None
                }
            }
            None => {
                map.insert(key.to_owned(), (now, 1));
                Some(1)
            }
        }
    }

    async fn send_pong(&self, link: Option<&Arc<Link>>) {
        if let Some(link) = link.filter(|l| l.is_open()) {
            let _ = self.send_text(link, frames::PONG_FRAME.to_owned()).await;
        }
    }

    fn abort_link(link: Option<&Arc<Link>>) {
        if let Some(link) = link {
            link.abort();
        }
    }

    fn settle_welcome(link: Option<&Arc<Link>>) {
        if let Some(link) = link {
            link.settle_welcome();
        }
    }

    fn apply_first_party(
        &self,
        features: Option<&[String]>,
        hosts: Option<&[String]>,
        paths: Option<&[String]>,
    ) {
        if has_feature(features, feature::WELCOME_HOSTS) {
            let policy = FirstPartyPolicy::with_server_provided(hosts, paths);
            *self.first_party.write().unwrap_or_else(|e| e.into_inner()) = policy;
        }
    }

    fn finish_resolve(&self, id: &str, resolved_url: Option<&str>, resolved: bool, result: MeshResult) {
        if id.is_empty() {
            return;
        }
        let cid = lock(&self.inflight_cids).remove(id);
        if resolved && let (Some(cid), Some(url)) = (cid.as_deref(), resolved_url) {
            self.remember_resolved_url_cid(url, cid);
        }
        let mut sink = lock(&self.pending);
        if let Some(tx) = sink.remove(id) {
            drop(sink);
            if resolved {
                self.reconnect_attempt.store(0, Ordering::SeqCst);
            }
            let _ = tx.send(result);
        }
    }

    async fn dispatch_text(self: &Arc<Self>, link: Option<&Arc<Link>>, payload: &[u8]) {
        let root: Value = match serde_json::from_slice(payload) {
            Ok(v) => v,
            Err(e) => {
                if let Some(count) = self.parse_failure_count("json") {
                    self.ux.warn(
                        Component::Mesh,
                        &format!(
                            "frame parse failed (json x{count} in last min): {} -- preview={}",
                            sanitize_for_console(&e.to_string(), 80),
                            payload_preview(payload, 120)
                        ),
                    );
                }
                return;
            }
        };
        let name = frames::str_field(&root, "action").unwrap_or_default().to_owned();
        match name.as_str() {
            action::RESOLVED | action::FALLBACK_NATIVE => {
                let resolved = name == action::RESOLVED;
                let id = frames::str_field(&root, "id").unwrap_or_default();
                let reason = frames::str_field(&root, "reason").map(str::to_owned);
                let retry_after_ms = if resolved {
                    None
                } else {
                    frames::i32_field(&root, "retry_after_ms")
                };
                let url = if resolved {
                    frames::str_field(&root, "url")
                } else {
                    None
                };
                if resolved && let Some(bytes) = root.get("bytes_estimate").and_then(Value::as_i64) {
                    self.stats.record_bytes_estimate(bytes);
                }
                if !resolved {
                    self.log
                        .line(&frames::fallback_native_line(id, reason.as_deref()));
                }
                let result = MeshResult {
                    frame: payload.to_vec(),
                    action: name.clone(),
                    reason,
                    retry_after_ms,
                };
                self.finish_resolve(id, url.filter(|u| !u.is_empty()), resolved, result);
            }
            action::WELCOME => self.on_welcome(link, payload),
            action::WELCOME_CACHED => self.on_welcome_cached(link, payload),
            action::RESOLVE_LOG => self.log.line(&frames::resolve_log_line(&root)),
            action::PLAYBACK_QUALITY_UPDATE => self.log.line(&frames::playback_quality_line(&root)),
            action::RATE_LIMITED => self.on_rate_limited(&root),
            action::PROTOCOL_ERROR => self.on_protocol_error(&root),
            action::PONG => *lock(&self.last_pong) = Some(Self::now()),
            action::PING => self.send_pong(link).await,
            other => self.ux.warn(
                Component::Mesh,
                &format!(
                    "unknown action -- discarding: {}",
                    sanitize_for_console(other, 64)
                ),
            ),
        }
    }

    fn on_welcome(&self, link: Option<&Arc<Link>>, payload: &[u8]) {
        let welcome: WelcomeFrame = match serde_json::from_slice(payload) {
            Ok(w) => w,
            Err(e) => {
                self.ux.warn(
                    Component::Mesh,
                    &format!(
                        "welcome parse failed -- assuming v1 server: {}",
                        sanitize_for_console(&e.to_string(), 160)
                    ),
                );
                {
                    let mut s = lock(&self.server);
                    if s.protocol_version == 0 {
                        s.protocol_version = 1;
                    }
                }
                Self::settle_welcome(link);
                return;
            }
        };
        let negotiated = welcome.protocol_version.clamp(1, wire::CLIENT_PROTOCOL_VERSION);
        let negotiated_format = welcome
            .negotiated_format
            .clone()
            .unwrap_or_else(|| format::JSON.to_owned());
        let is_msgpack = negotiated_format == format::MSGPACK;
        let (is_v3, host) = {
            let mut s = lock(&self.server);
            s.protocol_version = negotiated;
            s.node = welcome.node.clone();
            s.features = welcome.features.clone();
            s.warp_active = welcome.warp_active;
            s.server_version = welcome.server_version.clone();
            s.yt_dlp_version = welcome.yt_dlp_version.clone();
            s.negotiated_format = negotiated_format.clone();
            s.is_msgpack = is_msgpack;
            (s.is_v3, s.current_host.clone())
        };
        self.log.line(&format!(
            "[mesh][v3.1] negotiated_format={negotiated_format} isMsgpack={is_msgpack}"
        ));
        if welcome.features.is_none() {
            self.ux
                .warn(Component::Mesh, "welcome missing required field: features");
        }
        self.apply_first_party(
            welcome.features.as_deref(),
            welcome.first_party_hosts.as_deref(),
            welcome.playback_proxy_paths.as_deref(),
        );
        let features = match welcome.features.as_deref() {
            Some(f) if !f.is_empty() => format!("[{}]", f.join(",")),
            _ => "[]".to_owned(),
        };
        self.ux.write(
            Component::Mesh,
            &format!(
                "welcome node={} v={} (negotiated={negotiated}) server={} yt-dlp={} warp_active={} features={}",
                welcome.node.as_deref().unwrap_or("?"),
                welcome.protocol_version,
                welcome.server_version.as_deref().unwrap_or("?"),
                welcome.yt_dlp_version.as_deref().unwrap_or("?"),
                welcome.warp_active.map_or_else(|| "?".to_owned(), |w| w.to_string()),
                sanitize_for_console(&features, 240)
            ),
        );
        if is_v3
            && !host.is_empty()
            && let Some(hash) = welcome.welcome_hash.as_deref().filter(|h| !h.is_empty())
        {
            self.welcome_cache.store(&host, &welcome, hash);
        }
        Self::settle_welcome(link);
    }

    fn on_welcome_cached(&self, link: Option<&Arc<Link>>, payload: &[u8]) {
        let (is_v3, host) = {
            let s = lock(&self.server);
            (s.is_v3, s.current_host.clone())
        };
        if !is_v3 {
            self.ux.warn(
                Component::Mesh,
                "welcome_cached received on non-v3 connection -- protocol error, reconnecting",
            );
            Self::abort_link(link);
            return;
        }
        let cached: WelcomeCachedFrame = match serde_json::from_slice(payload) {
            Ok(c) => c,
            Err(e) => {
                self.ux.warn(
                    Component::Mesh,
                    &format!(
                        "welcome_cached parse failed: {}",
                        sanitize_for_console(&e.to_string(), 160)
                    ),
                );
                {
                    let mut s = lock(&self.server);
                    if s.protocol_version == 0 {
                        s.protocol_version = 1;
                    }
                }
                Self::settle_welcome(link);
                return;
            }
        };
        let Some(entry) = self.welcome_cache.get(&host) else {
            self.welcome_cache.invalidate(&host);
            self.ux.warn(
                Component::Mesh,
                "welcome_cached but local entry missing -- invalidating + reconnecting",
            );
            Self::abort_link(link);
            return;
        };
        let negotiated = cached.protocol_version.clamp(1, wire::CLIENT_PROTOCOL_VERSION);
        let negotiated_format = cached
            .negotiated_format
            .clone()
            .unwrap_or_else(|| format::JSON.to_owned());
        let node = cached.node.clone().or_else(|| entry.node.clone());
        {
            let mut s = lock(&self.server);
            s.protocol_version = negotiated;
            s.node = node.clone();
            s.features = entry.features.clone();
            s.warp_active = cached.warp_active.or(entry.warp_active);
            s.server_version = entry.server_version.clone();
            s.yt_dlp_version = entry.yt_dlp_version.clone();
            s.is_msgpack = negotiated_format == format::MSGPACK;
            s.negotiated_format = negotiated_format.clone();
        }
        self.apply_first_party(
            entry.features.as_deref(),
            entry.first_party_hosts.as_deref(),
            entry.playback_proxy_paths.as_deref(),
        );
        self.log.line(&format!(
            "[mesh][v3] welcome_cached hit node={} v={negotiated} negotiated_format={negotiated_format} features={}",
            node.as_deref().unwrap_or("?"),
            entry
                .features
                .as_deref()
                .map_or_else(|| "<none>".to_owned(), |f| f.join(","))
        ));
        Self::settle_welcome(link);
    }

    fn on_rate_limited(&self, root: &Value) {
        let mesh_action = frames::str_field(root, wire::field::MESH_ACTION).unwrap_or_default();
        let limited_id = frames::str_field(root, "id").filter(|i| !i.is_empty());
        let retry_after = frames::i32_field(root, wire::field::RETRY_AFTER_SECONDS).unwrap_or(0);
        let cooldown = retry_after.clamp(1, MAX_RATE_LIMIT_COOLDOWN_SECONDS);
        self.ux.warn(
            Component::Mesh,
            &format!(
                "server rate-limited {} -- pausing for {cooldown}s",
                sanitize_for_console(mesh_action, 32)
            ),
        );
        if mesh_action != action::RESOLVE {
            return;
        }
        let secs = u64::try_from(cooldown).unwrap_or(1);
        *lock(&self.rate_limited_until) = Some(Self::now() + Duration::from_secs(secs));
        match limited_id {
            Some(id) => {
                lock(&self.inflight_cids).remove(id);
                self.complete(id, frames::fallback_result(id, fallback::RATE_LIMITED));
            }
            None => self.fail_all_pending(fallback::RATE_LIMITED),
        }
    }

    fn on_protocol_error(&self, root: &Value) {
        let reason = frames::str_field(root, "reason").unwrap_or_default();
        let field = frames::str_field(root, "field").unwrap_or_default();
        let id = frames::str_field(root, "id");
        let mut line = format!(
            "server protocol_error reason={}",
            sanitize_for_console(reason, 48)
        );
        if !field.is_empty() {
            line.push_str(&format!(" field={}", sanitize_for_console(field, 48)));
        }
        if let Some(id) = id {
            line.push_str(&format!(" id={}", sanitize_for_console(id, 32)));
        }
        self.ux.warn(Component::Mesh, &line);
        if let Some(id) = id.filter(|i| !i.is_empty()) {
            lock(&self.inflight_cids).remove(id);
            self.complete(id, frames::fallback_result(id, fallback::PROTOCOL_ERROR));
        }
    }

    async fn dispatch_binary(self: &Arc<Self>, link: Option<&Arc<Link>>, payload: &[u8]) {
        let frame = match frames::decode_binary(payload) {
            Ok(f) => f,
            Err(detail) => {
                let key = format!("binary:{}", detail.split(':').next().unwrap_or_default());
                if let Some(count) = self.parse_failure_count(&key) {
                    self.ux.warn(
                        Component::Mesh,
                        &format!(
                            "binary frame parse failed ({key} x{count} in last min): {} -- preview={}",
                            sanitize_for_console(&detail, 200),
                            payload_preview(payload, 60)
                        ),
                    );
                }
                return;
            }
        };
        match frame {
            BinaryFrame::Resolved(resp) => {
                if resp.id.is_empty() {
                    return;
                }
                if let Some(bytes) = resp.bytes_estimate {
                    self.stats.record_bytes_estimate(bytes);
                }
                let result = MeshResult {
                    frame: json_frame(&resp),
                    action: action::RESOLVED.to_owned(),
                    reason: None,
                    retry_after_ms: None,
                };
                let url = resp.url.as_deref().filter(|u| !u.is_empty());
                self.finish_resolve(&resp.id, url, true, result);
            }
            BinaryFrame::FallbackNative(resp) => {
                if resp.id.is_empty() {
                    return;
                }
                self.log
                    .line(&frames::fallback_native_line(&resp.id, resp.reason.as_deref()));
                let result = MeshResult {
                    frame: json_frame(&resp),
                    action: action::FALLBACK_NATIVE.to_owned(),
                    reason: resp.reason.clone(),
                    retry_after_ms: resp.retry_after_ms,
                };
                self.finish_resolve(&resp.id, None, false, result);
            }
            BinaryFrame::ResolveLog { id, message } => self.log.line(&format!(
                "[mesh][resolve_log] id={} {}",
                sanitize_for_console(&id, 32),
                sanitize_for_console(&message, 240)
            )),
            BinaryFrame::Pong => {
                *lock(&self.last_pong) = Some(Self::now());
                self.log
                    .line("[mesh] pong received via binary path (server should send as Text per v3.1 spec)");
            }
            BinaryFrame::Ping => {
                self.log
                    .line("[mesh] ping received via binary path (server should send as Text per v3.1 spec)");
                self.send_pong(link).await;
            }
            BinaryFrame::Other(name) if name == action::PROTOCOL_ERROR || name == action::RATE_LIMITED => {
                self.ux.warn(
                    Component::Mesh,
                    &format!("{name} received via binary path (server should send as Text per v3.1 spec)"),
                );
            }
            BinaryFrame::Other(name) => self.ux.warn(
                Component::Mesh,
                &format!(
                    "unknown binary action -- discarding: {}",
                    sanitize_for_console(&name, 64)
                ),
            ),
        }
    }
}

fn json_frame(resp: &ResolveResponse) -> Vec<u8> {
    serde_json::to_vec(resp).unwrap_or_default()
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
