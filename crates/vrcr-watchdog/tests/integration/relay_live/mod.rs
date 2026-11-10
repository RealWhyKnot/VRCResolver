use std::fmt::Write as _;

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
    HitchDetector, RelayActivity, RelayConfig, RelayServer, Upstream, UpstreamError,
    UpstreamRequest, UpstreamResponse,
};
use vrcr_watchdog::stats::Stats;

use crate::common::mesh::Capture;

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
        OpenGuard(std::sync::Arc::clone(counter))
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
        map.append(
            *k,
            HeaderValue::from_str(v).unwrap_or_else(|e| panic!("{e}")),
        );
    }
    UpstreamResponse {
        direct: false,
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
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Seen {
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
        let file = req
            .url
            .path()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
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
        recovery: Default::default(),
        upstream,
        policy: Arc::new(RwLock::new(FirstPartyPolicy::default())),
        activity: std::sync::Arc::clone(&activity),
        hitch: Arc::new(HitchDetector::new()),
        stats: std::sync::Arc::clone(&stats),
        manifests: Arc::default(),
        now: clock::system(),
        ux: Ux::new(Box::new(out.clone()), Style::plain(), String::new),
        log: Log::disabled(),
    };
    (deps, stats, activity)
}

async fn harness(timeout: Option<Duration>) -> Harness {
    let script = Arc::new(Script::default());
    let out = Capture::default();
    let (deps, stats, activity) = deps(Arc::<Script>::clone(&script), &out);
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
            let _ = write!(text, "{k}: {v}\r\n");
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
                reply
                    .headers
                    .push((k.trim().to_owned(), v.trim().to_owned()));
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
                let size =
                    usize::from_str_radix(size.split(';').next().unwrap_or_default().trim(), 16)
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

fn tls() -> rustls::ClientConfig {
    (*platform_tls().unwrap_or_else(|e| panic!("{e}"))).clone()
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
    let log = std::sync::Arc::clone(&heads);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let log = std::sync::Arc::clone(&log);
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

mod contracts_cases;

mod lifecycle_cases;

mod local_cases;

mod manifests_cases;

mod live_playlist_cases;
mod live_streaming_cases;

mod streaming_cases;

mod seek_abort_cases;

mod diagnostics_cases;
mod retry_cases;

mod retry_transport_cases;
