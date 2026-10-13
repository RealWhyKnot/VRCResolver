use std::collections::VecDeque;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Bytes, Message};

use vrcr_shared::console::{Style, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::wire::{SUBPROTOCOL_V3, action};
use vrcr_watchdog::mesh::{Config, MeshClient};
use vrcr_watchdog::stats::Stats;

pub const MEDIA: &str = "https://media.example/v.mp4";
pub const DEFAULT_WELCOME: &str = r#"{"action":"welcome","protocol_version":1,"node":"fake"}"#;
const WELCOME_THEN_CLOSE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Accept,
    Refuse,
    Hang,
    SlowAccept,
    WelcomeThenClose,
    Blackhole,
}

enum Cmd {
    Text(String),
    Binary(Vec<u8>),
    Close,
    Abort,
}

#[derive(Clone)]
pub struct Conn {
    pub index: u32,
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Conn {
    pub fn text(&self, s: impl Into<String>) {
        let _ = self.tx.send(Cmd::Text(s.into()));
    }

    pub fn binary(&self, b: Vec<u8>) {
        let _ = self.tx.send(Cmd::Binary(b));
    }

    pub fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }

    pub fn abort(&self) {
        let _ = self.tx.send(Cmd::Abort);
    }

    pub fn resolved(&self, id: &str) {
        self.text(format!(r#"{{"action":"resolved","id":"{id}","url":"{MEDIA}"}}"#));
    }
}

pub type BoxFut = Pin<Box<dyn Future<Output = ()> + Send>>;
pub type Handler = Arc<dyn Fn(Conn, String) -> BoxFut + Send + Sync>;
pub type Policy = Arc<dyn Fn(u32) -> Mode + Send + Sync>;
pub type Welcome = Arc<dyn Fn(u32, Option<&Value>) -> String + Send + Sync>;

pub fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(Conn, String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Arc::new(move |c, id| Box::pin(f(c, id)))
}

pub fn policy(f: impl Fn(u32) -> Mode + Send + Sync + 'static) -> Policy {
    Arc::new(f)
}

pub struct Options {
    pub policy: Policy,
    pub on_resolve: Handler,
    pub on_connect: Option<Handler>,
    pub welcome: Welcome,
    pub v3: bool,
    pub slow_accept: Duration,
    pub close_after: Arc<dyn Fn(u32) -> Option<Duration> + Send + Sync>,
    pub gate_later_connections: bool,
    pub on_attempt: Option<Arc<dyn Fn(u32) + Send + Sync>>,
}

impl Options {
    pub fn new(policy: Policy) -> Options {
        Options {
            policy,
            on_resolve: handler(|c: Conn, id: String| async move { c.resolved(&id) }),
            on_connect: None,
            welcome: Arc::new(|_, _| DEFAULT_WELCOME.to_owned()),
            v3: false,
            slow_accept: Duration::from_secs(2),
            close_after: Arc::new(|_| None),
            gate_later_connections: false,
            on_attempt: None,
        }
    }

    pub fn on_resolve(mut self, h: Handler) -> Options {
        self.on_resolve = h;
        self
    }

    pub fn silent_resolve(self) -> Options {
        self.on_resolve(handler(|_c: Conn, _id: String| async {}))
    }
}

#[derive(Default)]
pub struct Record {
    pub paths: Vec<String>,
    pub user_agents: Vec<String>,
    pub resolves: Vec<(u32, String)>,
    pub request_json: Vec<String>,
    pub pings: Vec<u32>,
    pub texts: Vec<(u32, Value)>,
}

pub struct Server {
    pub port: u16,
    attempts: Arc<AtomicU32>,
    connections: Arc<AtomicU32>,
    record: Arc<Mutex<Record>>,
    gate: watch::Sender<bool>,
    accept_loop: Mutex<Option<JoinHandle<()>>>,
}

struct Shared {
    opts: Options,
    attempts: Arc<AtomicU32>,
    connections: Arc<AtomicU32>,
    record: Arc<Mutex<Record>>,
    gate: watch::Receiver<bool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    pub async fn start(opts: Options) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let port = listener.local_addr().unwrap_or_else(|e| panic!("{e}")).port();
        let attempts = Arc::new(AtomicU32::new(0));
        let connections = Arc::new(AtomicU32::new(0));
        let record = Arc::new(Mutex::new(Record::default()));
        let (gate, gate_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            opts,
            attempts: Arc::clone(&attempts),
            connections: Arc::clone(&connections),
            record: Arc::clone(&record),
            gate: gate_rx,
        });
        let accept_loop = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let attempt = shared.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(f) = &shared.opts.on_attempt {
                    f(attempt);
                }
                tokio::spawn(Arc::clone(&shared).handle(stream, attempt));
            }
        });
        Server {
            port,
            attempts,
            connections,
            record,
            gate,
            accept_loop: Mutex::new(Some(accept_loop)),
        }
    }

    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/mesh", self.port)
    }

    pub fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::SeqCst)
    }

    pub fn connections(&self) -> u32 {
        self.connections.load(Ordering::SeqCst)
    }

    pub fn record<R>(&self, f: impl FnOnce(&Record) -> R) -> R {
        f(&lock(&self.record))
    }

    pub fn resolves(&self) -> Vec<(u32, String)> {
        self.record(|r| r.resolves.clone())
    }

    pub fn open_gate(&self) {
        self.gate.send_replace(true);
    }

    pub fn stop_accepting(&self) {
        if let Some(h) = lock(&self.accept_loop).take() {
            h.abort();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop_accepting();
    }
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn header<'a>(head: &'a str, name: &str) -> &'a str {
    head.lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
        .map_or("", |(_, v)| v.trim())
}

impl Shared {
    fn note_request(&self, path: &str, ua: &str) {
        let mut r = lock(&self.record);
        r.paths.push(path.to_owned());
        r.user_agents.push(ua.to_owned());
    }

    async fn handle(self: Arc<Self>, mut stream: TcpStream, attempt: u32) {
        let mode = (self.opts.policy)(attempt);
        match mode {
            Mode::Refuse | Mode::Hang => {
                let head = read_head(&mut stream).await;
                let path = head.split_whitespace().nth(1).unwrap_or_default().to_owned();
                self.note_request(&path, header(&head, "user-agent"));
                if mode == Mode::Refuse {
                    let _ = stream
                        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    let _ = stream.shutdown().await;
                } else {
                    std::future::pending::<()>().await;
                }
                return;
            }
            Mode::SlowAccept => tokio::time::sleep(self.opts.slow_accept).await,
            _ => {}
        }
        if self.opts.gate_later_connections && self.connections.load(Ordering::SeqCst) >= 1 {
            let mut gate = self.gate.clone();
            let _ = gate.wait_for(|g| *g).await;
        }
        let v3 = self.opts.v3;
        let me = Arc::clone(&self);
        #[allow(clippy::result_large_err)]
        let callback = move |req: &Request, mut resp: Response| {
            let ua = req
                .headers()
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            me.note_request(req.uri().path(), ua);
            let asked_v3 = req
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|p| p.split(',').any(|x| x.trim() == SUBPROTOCOL_V3));
            if v3 && asked_v3 {
                resp.headers_mut()
                    .insert("sec-websocket-protocol", HeaderValue::from_static(SUBPROTOCOL_V3));
            }
            Ok(resp)
        };
        let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
            return;
        };
        let index = self.connections.fetch_add(1, Ordering::SeqCst) + 1;
        let close_after = if mode == Mode::WelcomeThenClose {
            Some(WELCOME_THEN_CLOSE)
        } else {
            (self.opts.close_after)(attempt)
        };
        self.serve(index, ws, close_after, mode == Mode::Blackhole).await;
    }

    async fn serve(
        self: Arc<Self>,
        index: u32,
        ws: tokio_tungstenite::WebSocketStream<TcpStream>,
        close_after: Option<Duration>,
        silent: bool,
    ) {
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let conn = Conn { index, tx };
        let mut queued: VecDeque<Value> = VecDeque::new();
        if self.opts.v3 {
            loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = serde_json::from_str(t.as_str()).unwrap_or(Value::Null);
                        lock(&self.record).texts.push((index, v.clone()));
                        if v["action"] == action::CLIENT_HELLO {
                            let welcome = (self.opts.welcome)(index, Some(&v));
                            conn.text(welcome);
                            break;
                        }
                        queued.push_back(v);
                    }
                    Some(Ok(_)) => {}
                    _ => return,
                }
            }
        } else {
            conn.text((self.opts.welcome)(index, None));
        }
        if let Some(h) = &self.opts.on_connect {
            h(conn.clone(), String::new()).await;
        }
        if let Some(delay) = close_after {
            let c = conn.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                c.close();
            });
        }
        for v in queued.drain(..) {
            self.on_text(&conn, v, silent, false).await;
        }
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let sent = match cmd {
                        None | Some(Cmd::Abort) => return,
                        Some(Cmd::Text(t)) => sink.send(Message::text(t)).await,
                        Some(Cmd::Binary(b)) => sink.send(Message::Binary(Bytes::from(b))).await,
                        Some(Cmd::Close) => sink.send(Message::Close(Some(CloseFrame {
                            code: CloseCode::Normal,
                            reason: "scripted close".into(),
                        }))).await,
                    };
                    if sent.is_err() {
                        return;
                    }
                }
                msg = stream.next() => {
                    let text = match msg {
                        Some(Ok(Message::Text(t))) => t,
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                        Some(Ok(_)) => continue,
                    };
                    let v: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    self.on_text(&conn, v, silent, true).await;
                }
            }
        }
    }

    async fn on_text(&self, conn: &Conn, v: Value, silent: bool, record_text: bool) {
        if record_text {
            lock(&self.record).texts.push((conn.index, v.clone()));
        }
        let name = v["action"].as_str().unwrap_or_default();
        if name == action::PING {
            lock(&self.record).pings.push(conn.index);
            if !silent {
                conn.text(r#"{"action":"pong"}"#);
            }
            return;
        }
        if name != action::RESOLVE {
            return;
        }
        let id = v["id"].as_str().unwrap_or_default().to_owned();
        {
            let mut r = lock(&self.record);
            r.resolves.push((conn.index, id.clone()));
            r.request_json.push(v.to_string());
        }
        if !silent {
            (self.opts.on_resolve)(conn.clone(), id).await;
        }
    }
}

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&lock(&self.0)).into_owned()
    }
}

pub struct TestClient {
    pub client: MeshClient,
    pub out: Capture,
    pub hosts: Arc<Mutex<Vec<String>>>,
    pub dir: TempDir,
}

impl TestClient {
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("watchdog.log")).unwrap_or_default()
    }
}

pub fn client_for(server: &Server, tweak: impl FnOnce(&mut Config)) -> TestClient {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    client_in(dir, server, tweak)
}

pub fn client_in(dir: TempDir, server: &Server, tweak: impl FnOnce(&mut Config)) -> TestClient {
    let out = Capture::default();
    let hosts = Arc::new(Mutex::new(Vec::new()));
    let mut cfg = Config::new(dir.path(), "test");
    let url = server.url();
    let seen = Arc::clone(&hosts);
    cfg.url_for_host = Arc::new(move |host: &str| {
        lock(&seen).push(host.to_owned());
        url.clone()
    });
    tweak(&mut cfg);
    let ux = Ux::new(Box::new(out.clone()), Style::plain(), String::new);
    let client = MeshClient::new(
        cfg,
        ux,
        Log::to_file(&dir.path().join("watchdog.log")),
        Arc::new(Stats::new(std::time::SystemTime::now())),
    );
    TestClient {
        client,
        out,
        hosts,
        dir,
    }
}

pub fn fixed_backoff(ms: u64) -> Arc<dyn Fn(u32) -> u64 + Send + Sync> {
    Arc::new(move |_| ms)
}

pub async fn wait_until(cond: impl Fn() -> bool, timeout_ms: u64, what: &str) {
    let started = std::time::Instant::now();
    while !cond() {
        assert!(
            started.elapsed() < Duration::from_millis(timeout_ms),
            "{what} not reached within {timeout_ms} ms"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
