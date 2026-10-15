mod common;

use std::collections::VecDeque;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use futures_util::Stream;
use http::header::HeaderValue;
use http::{HeaderMap, Method, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_watchdog::clock;
use vrcr_watchdog::mesh::connect::platform_tls;
use vrcr_watchdog::relay::server::RelayDeps;
use vrcr_watchdog::relay::targets::encode_target_param;
use vrcr_watchdog::relay::upstream::{ReqwestUpstream, Sending};
use vrcr_watchdog::relay::{
    HitchDetector, RelayActivity, RelayConfig, RelayServer, Upstream, UpstreamError, UpstreamRequest,
    UpstreamResponse,
};
use vrcr_watchdog::stats::Stats;

use common::mesh::Capture;

const WITHIN: Duration = Duration::from_secs(5);
const MANIFEST: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nhttps://us1.vrcresolver.com/api/proxy/hls/seg_000001.ts?playback_id=t\n#EXTINF:2,\nhttps://us1.vrcresolver.com/api/proxy/hls/seg_000002.ts?playback_id=t\n#EXTINF:2,\nseg_000003.ts\n#EXT-X-ENDLIST\n";
static ZEROS: [u8; 65536] = [0; 65536];

#[derive(Clone)]
struct Seen {
    method: Method,
    url: String,
    headers: HeaderMap,
}

#[derive(Default)]
struct Script {
    seen: Mutex<Vec<Seen>>,
    open: Arc<AtomicUsize>,
}

impl Script {
    fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    fn snapshot(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn seen_urls(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|s| s.url.clone())
            .collect()
    }
}

struct OpenGuard(Arc<AtomicUsize>);

impl OpenGuard {
    fn new(counter: &Arc<AtomicUsize>) -> OpenGuard {
        counter.fetch_add(1, Ordering::SeqCst);
        OpenGuard(counter.clone())
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

enum Tail {
    End,
    Pending,
    Endless,
}

struct ScriptBody {
    chunks: VecDeque<io::Result<Bytes>>,
    tail: Tail,
    _open: OpenGuard,
}

impl Stream for ScriptBody {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(chunk) = self.chunks.pop_front() {
            return Poll::Ready(Some(chunk));
        }
        match self.tail {
            Tail::End => Poll::Ready(None),
            Tail::Pending => Poll::Pending,
            Tail::Endless => Poll::Ready(Some(Ok(Bytes::from_static(&ZEROS)))),
        }
    }
}

fn respond(
    open: &Arc<AtomicUsize>,
    status: u16,
    headers: &[(&'static str, String)],
    chunks: Vec<io::Result<Bytes>>,
    tail: Tail,
) -> UpstreamResponse {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        map.append(*k, HeaderValue::from_str(v).unwrap_or_else(|e| panic!("{e}")));
    }
    UpstreamResponse {
        status: StatusCode::from_u16(status).unwrap_or_else(|e| panic!("{e}")),
        headers: map,
        body: Box::pin(ScriptBody {
            chunks: chunks.into(),
            tail,
            _open: OpenGuard::new(open),
        }),
    }
}

fn filled(len: usize, byte: u8) -> Vec<io::Result<Bytes>> {
    let mut out = Vec::new();
    let mut left = len;
    while left > 0 {
        let n = left.min(16 * 1024);
        out.push(Ok(Bytes::from(vec![byte; n])));
        left -= n;
    }
    out
}

impl Upstream for Script {
    fn send(&self, req: UpstreamRequest) -> Sending<'_> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).push(Seen {
            method: req.method.clone(),
            url: req.url.to_string(),
            headers: req.headers.clone(),
        });
        Box::pin(async move { self.script(&req).await })
    }
}

impl Script {
    async fn script(&self, req: &UpstreamRequest) -> Result<UpstreamResponse, UpstreamError> {
        let open = &self.open;
        let mut parts = req.url.path().split('/').skip(3);
        let mode = parts.next().unwrap_or_default().to_owned();
        let file = req.url.path().rsplit('/').next().unwrap_or_default().to_owned();
        let range = req.headers.get("range").is_some();
        let ts = || ("content-type", "video/mp2t".to_owned());
        let r = match mode.as_str() {
            m if m.starts_with("full-") => {
                let len: usize = m[5..].parse().unwrap_or_else(|e| panic!("{e}"));
                if range {
                    respond(
                        open,
                        206,
                        &[
                            ("content-range", format!("bytes 0-99/{len}")),
                            ("content-length", "100".to_owned()),
                            ts(),
                        ],
                        filled(100, b'r'),
                        Tail::End,
                    )
                } else {
                    respond(
                        open,
                        200,
                        &[
                            ("content-length", len.to_string()),
                            ts(),
                            ("cf-ray", "abc".to_owned()),
                            ("x-lazy-hls", "HIT".to_owned()),
                        ],
                        filled(len, b'v'),
                        Tail::End,
                    )
                }
            }
            "short" => respond(
                open,
                200,
                &[("content-length", "1000".to_owned()), ts()],
                filled(500, b's'),
                Tail::End,
            ),
            "throw" => {
                let mut chunks = filled(500, b't');
                chunks.push(Err(io::Error::other("boom")));
                respond(
                    open,
                    200,
                    &[("content-length", "1000".to_owned()), ts()],
                    chunks,
                    Tail::End,
                )
            }
            "stall-body" => respond(
                open,
                200,
                &[("content-length", "1000".to_owned()), ts()],
                filled(500, b'p'),
                Tail::Pending,
            ),
            "stall-headers" => std::future::pending().await,
            "endless" => respond(open, 200, &[ts()], Vec::new(), Tail::Endless),
            "fail" => return Err(UpstreamError::new("connect_error", "refused")),
            "status-404" | "status-500" => {
                let code = mode[7..].parse().unwrap_or_else(|e| panic!("{e}"));
                respond(
                    open,
                    code,
                    &[("content-length", "4".to_owned())],
                    vec![Ok(Bytes::from_static(b"nope"))],
                    Tail::End,
                )
            }
            "hls" if file.ends_with(".m3u8") => respond(
                open,
                200,
                &[("content-type", "application/vnd.apple.mpegurl".to_owned())],
                MANIFEST
                    .as_bytes()
                    .chunks(7)
                    .map(|c| Ok(Bytes::copy_from_slice(c)))
                    .collect(),
                Tail::End,
            ),
            "hls" => respond(
                open,
                200,
                &[("content-length", "1000".to_owned()), ts()],
                filled(1000, b'h'),
                Tail::End,
            ),
            "manifest-stall" => respond(
                open,
                200,
                &[("content-type", "application/vnd.apple.mpegurl".to_owned())],
                vec![Ok(Bytes::from_static(b"#EXTM3U\n"))],
                Tail::Pending,
            ),
            other => panic!("unknown mode {other}"),
        };
        Ok(r)
    }
}

struct Harness {
    server: RelayServer,
    script: Arc<Script>,
    out: Capture,
    stats: Arc<Stats>,
    activity: Arc<RelayActivity>,
}

fn deps(upstream: Arc<dyn Upstream>, out: &Capture) -> (RelayDeps, Arc<Stats>, Arc<RelayActivity>) {
    let stats = Arc::new(Stats::new(SystemTime::now()));
    let activity = Arc::new(RelayActivity::new());
    let deps = RelayDeps {
        upstream,
        policy: Arc::new(RwLock::new(FirstPartyPolicy::default())),
        activity: activity.clone(),
        hitch: Arc::new(HitchDetector::new()),
        stats: stats.clone(),
        now: clock::system(),
        ux: Ux::new(Box::new(out.clone()), Style::plain(), String::new),
        log: Log::disabled(),
    };
    (deps, stats, activity)
}

async fn harness(timeout: Option<Duration>) -> Harness {
    let script = Arc::new(Script::default());
    let out = Capture::default();
    let (deps, stats, activity) = deps(script.clone(), &out);
    let mut cfg = RelayConfig::http(0);
    cfg.timeout = timeout;
    let server = RelayServer::start(cfg, deps)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    Harness {
        server,
        script,
        out,
        stats,
        activity,
    }
}

fn upstream_url(mode: &str, file: &str) -> String {
    format!("https://us1.vrcresolver.com/api/proxy/{mode}/{file}")
}

fn play(id: &str, file: &str, target: &str) -> String {
    format!("/play/{id}/{file}?target={}", encode_target_param(target))
}

async fn eventually(check: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    complete: bool,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

struct Conn {
    stream: TcpStream,
    buf: Vec<u8>,
    port: u16,
}

impl Conn {
    async fn open(port: u16) -> Conn {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        Conn {
            stream,
            buf: Vec::new(),
            port,
        }
    }

    async fn send_raw(&mut self, text: &str) {
        self.stream
            .write_all(text.as_bytes())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }

    async fn send(&mut self, method: &str, path: &str, extra: &[(&str, &str)]) {
        let mut text = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost.youtube.com:{}\r\n",
            self.port
        );
        for (k, v) in extra {
            text.push_str(&format!("{k}: {v}\r\n"));
        }
        text.push_str("\r\n");
        self.send_raw(&text).await;
    }

    async fn get(&mut self, path: &str) -> Reply {
        self.send("GET", path, &[]).await;
        self.reply(false).await
    }

    async fn fill(&mut self) -> bool {
        let mut chunk = vec![0u8; 65536];
        match self.stream.read(&mut chunk).await {
            Ok(0) | Err(_) => false,
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                true
            }
        }
    }

    async fn need(&mut self, n: usize) -> bool {
        while self.buf.len() < n {
            if !self.fill().await {
                return false;
            }
        }
        true
    }

    async fn line(&mut self) -> Option<String> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.buf[..i]).into_owned();
                self.buf.drain(..i + 2);
                return Some(line);
            }
            if !self.fill().await {
                return None;
            }
        }
    }

    async fn reply(&mut self, head: bool) -> Reply {
        tokio::time::timeout(WITHIN, self.read_reply(head))
            .await
            .unwrap_or_else(|_| panic!("no reply within {WITHIN:?}"))
    }

    async fn read_reply(&mut self, head: bool) -> Reply {
        let mut reply = Reply {
            status: 0,
            headers: Vec::new(),
            body: Vec::new(),
            complete: false,
        };
        let Some(status) = self.line().await else {
            return reply;
        };
        reply.status = status
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("bad status line {status}"));
        loop {
            let Some(line) = self.line().await else {
                return reply;
            };
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                reply.headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
        if head || reply.status == 204 || reply.status == 304 {
            reply.complete = true;
            return reply;
        }
        if reply
            .header("transfer-encoding")
            .is_some_and(|t| t.eq_ignore_ascii_case("chunked"))
        {
            loop {
                let Some(size) = self.line().await else {
                    return reply;
                };
                let size = usize::from_str_radix(size.split(';').next().unwrap_or_default().trim(), 16)
                    .unwrap_or_else(|e| panic!("{e}"));
                if size == 0 {
                    while let Some(trailer) = self.line().await {
                        if trailer.is_empty() {
                            reply.complete = true;
                            return reply;
                        }
                    }
                    return reply;
                }
                if !self.need(size + 2).await {
                    let n = self.buf.len().min(size);
                    reply.body.extend(self.buf.drain(..n));
                    return reply;
                }
                reply.body.extend(self.buf.drain(..size));
                self.buf.drain(..2);
            }
        }
        if let Some(len) = reply
            .header("content-length")
            .and_then(|l| l.parse::<usize>().ok())
        {
            let ok = self.need(len).await;
            let n = self.buf.len().min(len);
            reply.body.extend(self.buf.drain(..n));
            reply.complete = ok;
            return reply;
        }
        while self.fill().await {}
        reply.body = std::mem::take(&mut self.buf);
        reply.complete = true;
        reply
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_rejects_before_touching_upstream() {
    let h = harness(None).await;
    let port = h.server.port();
    let good = play("gate", "seg.ts", &upstream_url("full-10", "seg.ts"));
    let mut c = Conn::open(port).await;

    c.send("GET", &good, &[("Origin", "https://evil.example")]).await;
    assert_eq!(c.reply(false).await.status, 403);
    assert_eq!(c.get("/not-play/gate/seg.ts").await.status, 404);
    c.send("POST", &good, &[("Content-Length", "0")]).await;
    let r = c.reply(false).await;
    assert_eq!(r.status, 405);
    assert_eq!(r.header("allow"), Some("GET, HEAD"));
    assert_eq!(c.get("/play/gate/seg.ts").await.status, 400);
    let evil = play("gate", "seg.ts", "https://example.com/api/proxy/seg.ts");
    assert_eq!(c.get(&evil).await.status, 403);
    let file = play("gate", "seg.ts", "file:///C:/Windows/win.ini");
    assert_eq!(c.get(&file).await.status, 403);
    c.send_raw(&format!(
        "GET {good} HTTP/1.1\r\nHost: us1.vrcresolver.com:{port}\r\n\r\n"
    ))
    .await;
    assert_eq!(c.reply(false).await.status, 403);
    c.send_raw(&format!(
        "GET {good} HTTP/1.1\r\nHost: localhost.youtube.com:{}\r\n\r\n",
        port.wrapping_add(1)
    ))
    .await;
    assert_eq!(c.reply(false).await.status, 403);

    assert!(h.script.seen_urls().is_empty());
    assert_eq!(
        h.activity
            .wedged_port(&format!("https://localhost.youtube.com:{port}/play/gate/seg.ts")),
        None
    );
    assert_eq!(h.stats.activity().relay_requests, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_body_streams_with_cleaned_headers() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    c.send(
        "GET",
        &play("full", "seg.ts", &upstream_url("full-100000", "seg.ts")),
        &[
            ("Cookie", "session=secret"),
            ("Authorization", "Bearer secret"),
            ("Accept-Encoding", "gzip, br"),
            ("User-Agent", "AVProMobileVideo"),
        ],
    )
    .await;
    let r = c.reply(false).await;
    assert_eq!(r.status, 200);
    assert!(r.complete);
    assert_eq!(r.header("content-length"), Some("100000"));
    assert_eq!(r.header("content-type"), Some("video/mp2t"));
    assert_eq!(r.header("x-lazy-hls"), Some("HIT"));
    assert_eq!(r.header("cf-ray"), None);
    assert_eq!(r.body.len(), 100_000);
    assert!(r.body.iter().all(|b| *b == b'v'));

    let seen = h.script.snapshot();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, Method::GET);
    assert_eq!(seen[0].url, upstream_url("full-100000", "seg.ts"));
    assert!(seen[0].headers.get("cookie").is_none());
    assert!(seen[0].headers.get("authorization").is_none());
    assert!(seen[0].headers.get("host").is_none());
    assert_eq!(seen[0].headers["accept-encoding"], "identity");
    assert_eq!(seen[0].headers["user-agent"], "AVProMobileVideo");

    assert!(eventually(|| h.stats.activity().upstream_relay_bytes == 100_000, WITHIN).await);
    let a = h.stats.activity();
    assert_eq!((a.relay_requests, a.upstream_relay_requests), (1, 1));
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_and_head_keep_the_connection_usable() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    let path = play("rng", "video.mp4", &upstream_url("full-5000", "video.mp4"));

    c.send("GET", &path, &[("Range", "bytes=0-99")]).await;
    let r = c.reply(false).await;
    assert_eq!(r.status, 206);
    assert_eq!(r.header("content-range"), Some("bytes 0-99/5000"));
    assert_eq!(r.body.len(), 100);
    assert!(r.complete);

    c.send("HEAD", &path, &[]).await;
    let r = c.reply(true).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-length"), Some("5000"));

    let r = c.get(&path).await;
    assert_eq!((r.status, r.body.len(), r.complete), (200, 5000, true));

    let seen = h.script.snapshot();
    assert_eq!(seen[0].headers["range"], "bytes=0-99");
    assert_eq!(seen[1].method, Method::HEAD);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_upstream_bodies_end_the_client_response_quickly() {
    let h = harness(None).await;
    let port = h.server.port();
    for mode in ["short", "throw"] {
        let mut c = Conn::open(port).await;
        let started = Instant::now();
        let r = c.get(&play(mode, "seg.ts", &upstream_url(mode, "seg.ts"))).await;
        assert_eq!(r.status, 200, "{mode}");
        assert!(!r.complete, "{mode}");
        assert!(r.body.len() <= 500, "{mode}");
        assert!(started.elapsed() < WITHIN, "{mode}");
    }
    let mut c = Conn::open(port).await;
    let r = c
        .get(&play("ok", "seg.ts", &upstream_url("full-1000", "seg.ts")))
        .await;
    assert!(r.complete);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
    let text = h.out.text();
    assert!(text.contains("upstream ended early (500/1000 bytes)"), "{text}");
    assert!(text.contains("upstream_body_error: boom"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalls_and_failures_hit_the_timeouts() {
    let h = harness(Some(Duration::from_millis(500))).await;
    let port = h.server.port();

    let mut c = Conn::open(port).await;
    let r = c
        .get(&play("sh", "seg.ts", &upstream_url("stall-headers", "seg.ts")))
        .await;
    assert_eq!(r.status, 504);

    let mut c = Conn::open(port).await;
    let r = c
        .get(&play("sb", "seg.ts", &upstream_url("stall-body", "seg.ts")))
        .await;
    assert_eq!(r.status, 200);
    assert!(!r.complete);

    let mut c = Conn::open(port).await;
    let r = c
        .get(&play(
            "ms",
            "index.m3u8",
            &upstream_url("manifest-stall", "index.m3u8"),
        ))
        .await;
    assert_eq!(r.status, 200);
    assert!(!r.complete);

    let mut c = Conn::open(port).await;
    let r = c.get(&play("f", "seg.ts", &upstream_url("fail", "seg.ts"))).await;
    assert_eq!(r.status, 502);
    for code in [404u16, 500] {
        let mode = format!("status-{code}");
        let r = c.get(&play("s", "seg.ts", &upstream_url(&mode, "seg.ts"))).await;
        assert_eq!(
            (r.status, r.body.as_slice(), r.complete),
            (code, &b"nope"[..], true)
        );
    }

    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
    let text = h.out.text();
    assert!(
        text.contains("upstream timed out for https://us1.vrcresolver.com/api/proxy/stall-headers/seg.ts"),
        "{text}"
    );
    assert!(text.contains("stream idle timeout for"), "{text}");
    assert!(
        text.contains(
            "upstream timed out for https://us1.vrcresolver.com/api/proxy/manifest-stall/index.m3u8"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "request failed for https://us1.vrcresolver.com/api/proxy/fail/seg.ts: connect_error: refused"
        ),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_client_is_aborted_while_the_next_request_is_served() {
    let h = harness(Some(Duration::from_millis(500))).await;
    let port = h.server.port();
    let mut stalled = Conn::open(port).await;
    stalled
        .send(
            "GET",
            &play("st", "seg.ts", &upstream_url("endless", "seg.ts")),
            &[],
        )
        .await;
    assert!(eventually(|| h.script.open() == 1, WITHIN).await);

    let mut next = Conn::open(port).await;
    let r = next
        .get(&play("nx", "seg.ts", &upstream_url("full-1000", "seg.ts")))
        .await;
    assert_eq!((r.status, r.body.len(), r.complete), (200, 1000, true));

    assert!(eventually(|| h.script.open() == 0, Duration::from_secs(10)).await);
    assert!(
        h.out.text().contains("client write stalled for"),
        "{}",
        h.out.text()
    );
    let r = tokio::time::timeout(Duration::from_secs(10), stalled.read_reply(false))
        .await
        .unwrap_or_else(|_| panic!("stalled client never saw the reset"));
    assert!(!r.complete);
    assert!(eventually(|| h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_leaving_releases_the_upstream() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    c.send(
        "GET",
        &play("lv", "seg.ts", &upstream_url("endless", "seg.ts")),
        &[],
    )
    .await;
    assert!(c.fill().await);
    assert!(eventually(|| h.script.open() == 1, WITHIN).await);
    drop(c);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thirty_back_to_back_plays_share_one_connection() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    for i in 0..30 {
        let r = c
            .get(&play(
                &format!("p{i}"),
                "video.mp4",
                &upstream_url("full-20000", "video.mp4"),
            ))
            .await;
        assert_eq!(
            (r.status, r.body.len(), r.complete),
            (200, 20000, true),
            "play {i}"
        );
    }
    assert_eq!(h.script.seen_urls().len(), 30);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hls_manifest_then_every_segment() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    let r = c
        .get(&play("hls1", "manifest.m3u8", &upstream_url("hls", "index.m3u8")))
        .await;
    assert_eq!(r.status, 200);
    assert!(r.complete);
    assert_eq!(r.header("content-type"), Some("application/vnd.apple.mpegurl"));
    assert_eq!(r.header("content-length"), None);
    let text = String::from_utf8(r.body).unwrap_or_else(|e| panic!("{e}"));
    assert!(!text.contains("https://"), "{text}");
    let segments: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    assert_eq!(segments.len(), 3, "{text}");
    for seg in &segments {
        let r = c.get(&format!("/play/hls1/{seg}")).await;
        assert_eq!((r.status, r.body.len(), r.complete), (200, 1000, true), "{seg}");
    }
    let urls = h.script.seen_urls();
    assert_eq!(
        urls,
        vec![
            upstream_url("hls", "index.m3u8"),
            upstream_url("hls", "seg_000001.ts?playback_id=t"),
            upstream_url("hls", "seg_000002.ts?playback_id=t"),
            upstream_url("hls", "seg_000003.ts"),
        ]
    );
    assert!(h.server.remembered_bases() >= 1);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perf_window_counts_relay_traffic_only_when_enabled() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    let r = c
        .get(&play(
            "perf0",
            "manifest.m3u8",
            &upstream_url("hls", "index.m3u8"),
        ))
        .await;
    assert_eq!(r.status, 200);
    assert!(eventually(|| h.server.in_flight() == 0, WITHIN).await);
    assert!(!h.stats.perf.take().is_active());

    h.stats.perf.enable();
    let r = c
        .get(&play(
            "perf1",
            "manifest.m3u8",
            &upstream_url("hls", "index.m3u8"),
        ))
        .await;
    let text = String::from_utf8(r.body).unwrap_or_else(|e| panic!("{e}"));
    let segments: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    for seg in &segments {
        let r = c.get(&format!("/play/perf1/{seg}")).await;
        assert_eq!(r.status, 200, "{seg}");
    }
    c.send("HEAD", &format!("/play/perf1/{}", segments[0]), &[]).await;
    assert_eq!(c.reply(true).await.status, 200);
    assert!(eventually(|| h.server.in_flight() == 0, WITHIN).await);
    let mut snap = h.stats.perf.take();
    assert_eq!(snap.relay_requests, 4);
    assert_eq!(snap.relay_errors, 0);
    assert!(snap.relay_bytes >= 3000, "{}", snap.relay_bytes);
    assert_eq!(snap.segment_total.spread().map(|s| s.count), Some(3));
    assert_eq!(snap.segment_header.spread().map(|s| s.count), Some(3));
    assert_eq!(snap.manifest_total.spread().map(|s| s.count), Some(1));
    assert!(!h.stats.perf.take().is_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_mid_stream_drops_everything() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    c.send(
        "GET",
        &play("sd", "seg.ts", &upstream_url("endless", "seg.ts")),
        &[],
    )
    .await;
    assert!(c.fill().await);
    tokio::time::timeout(WITHIN, h.server.stop())
        .await
        .unwrap_or_else(|_| panic!("stop hung"));
    let r = tokio::time::timeout(Duration::from_secs(10), c.read_reply(false))
        .await
        .unwrap_or_else(|_| panic!("client never saw the close"));
    assert!(!r.complete);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, h.server.port()))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_without_a_certificate_refuses_to_start() {
    let out = Capture::default();
    let (deps, _, _) = deps(Arc::new(Script::default()), &out);
    let mut cfg = RelayConfig::http(0);
    cfg.scheme = "https".to_owned();
    assert!(RelayServer::start(cfg, deps).await.is_err());
    assert!(out.text().contains("no certificate loaded"), "{}", out.text());
}

fn tls() -> rustls::ClientConfig {
    (*platform_tls().unwrap_or_else(|e| panic!("{e}"))).clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reqwest_upstream_refuses_internal_targets() {
    let up = ReqwestUpstream::new(tls()).unwrap_or_else(|e| panic!("{e}"));
    for target in [
        "http://127.0.0.1/api/proxy/x.ts",
        "http://localhost:9/api/proxy/x.ts?sig=secret",
        "http://169.254.169.254/latest/meta-data",
        "http://10.0.0.5/api/proxy/x.ts",
        "http://[::1]/api/proxy/x.ts",
    ] {
        let req = UpstreamRequest {
            method: Method::GET,
            url: Url::parse(target).unwrap_or_else(|e| panic!("{e}")),
            headers: HeaderMap::new(),
        };
        match up.send(req).await {
            Ok(_) => panic!("{target} was not refused"),
            Err(e) => {
                assert_eq!(e.kind, "blocked_address", "{target}: {e}");
                assert!(e.message.contains("blocked internal address"), "{target}: {e}");
                assert!(!e.message.contains("/api/proxy"), "{target}: {e}");
            }
        }
    }
}

struct Stub {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<String>>>,
}

async fn stub_origin() -> Stub {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let addr = listener.local_addr().unwrap_or_else(|e| panic!("{e}"));
    let heads = Arc::new(Mutex::new(Vec::new()));
    let log = heads.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                log.lock().unwrap_or_else(|e| e.into_inner()).push(head);
                let port = addr.port();
                let reply = match path.as_str() {
                    "/api/proxy/full/video.mp4" => {
                        let mut r = b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 50000\r\nConnection: close\r\n\r\n".to_vec();
                        r.extend(std::iter::repeat_n(b'o', 50000));
                        r
                    }
                    "/api/proxy/redirect/x.ts" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/api/proxy/full/video.mp4\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    "/api/proxy/hop/x.ts" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://us1.vrcresolver.com:{port}/api/proxy/full/video.mp4\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = s.write_all(&reply).await;
                let _ = s.shutdown().await;
            });
        }
    });
    Stub { addr, heads }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_streams_through_the_real_http_client() {
    let stub = stub_origin().await;
    let up = ReqwestUpstream::with_overrides(tls(), &[("us1.vrcresolver.com", stub.addr)])
        .unwrap_or_else(|e| panic!("{e}"));
    let out = Capture::default();
    let (deps, stats, _) = deps(Arc::new(up), &out);
    let server = RelayServer::start(RelayConfig::http(0), deps)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let origin = format!("http://us1.vrcresolver.com:{}", stub.addr.port());
    let mut c = Conn::open(server.port()).await;

    c.send(
        "GET",
        &play("rq", "video.mp4", &format!("{origin}/api/proxy/full/video.mp4")),
        &[("Cookie", "session=secret")],
    )
    .await;
    let r = c.reply(false).await;
    assert_eq!((r.status, r.body.len(), r.complete), (200, 50000, true));
    assert_eq!(r.header("content-type"), Some("video/mp4"));

    let mut c = Conn::open(server.port()).await;
    let r = c
        .get(&play("rq", "x.ts", &format!("{origin}/api/proxy/hop/x.ts")))
        .await;
    assert_eq!((r.status, r.body.len()), (200, 50000));

    let mut c = Conn::open(server.port()).await;
    let r = c
        .get(&play("rq", "x.ts", &format!("{origin}/api/proxy/redirect/x.ts")))
        .await;
    assert_eq!(r.status, 502);
    let text = out.text();
    assert!(text.contains("blocked_address"), "{text}");
    assert!(text.contains("blocked internal address (127.0.0.1)"), "{text}");

    let heads = stub.heads.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(heads.len(), 4, "{heads:?}");
    let first = heads[0].to_ascii_lowercase();
    assert!(first.contains("accept-encoding: identity"), "{first}");
    assert!(!first.contains("cookie"), "{first}");
    assert!(
        first.contains(&format!("host: us1.vrcresolver.com:{}", stub.addr.port())),
        "{first}"
    );
    assert!(eventually(|| stats.activity().upstream_relay_bytes == 100_000, WITHIN).await);
    server.stop().await;
}
