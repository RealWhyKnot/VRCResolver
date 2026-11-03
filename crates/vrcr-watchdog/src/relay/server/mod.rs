use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::task::Poll;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use http::header::{ALLOW, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderValue, ORIGIN};
use http::{Method, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::trust_gateway;

use super::activity::RelayActivity;
use super::headers::{self, content_length};
use super::hitch::{self, HitchDetector, TimingSample};
use super::manifest::{self, ManifestSink};
use super::security;
use super::short_url;
use super::targets::{self, TargetResolver};
use super::upstream::{BodyStream, Upstream, UpstreamRequest, UpstreamResponse};
use crate::clock::Now;
use crate::stats::Stats;

pub const MAX_IN_FLIGHT: usize = 128;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

type Body = BoxBody<Bytes, io::Error>;

pub struct RelayConfig {
    pub port: u16,
    pub scheme: String,
    pub timeout: Option<Duration>,
    pub tls: Option<Arc<rustls::ServerConfig>>,
    pub verbose: bool,
}

impl RelayConfig {
    pub fn http(port: u16) -> RelayConfig {
        RelayConfig {
            port,
            scheme: "http".to_owned(),
            timeout: None,
            tls: None,
            verbose: false,
        }
    }
}

pub struct RelayDeps {
    pub upstream: Arc<dyn Upstream>,
    pub policy: Arc<RwLock<FirstPartyPolicy>>,
    pub activity: Arc<RelayActivity>,
    pub hitch: Arc<HitchDetector>,
    pub stats: Arc<Stats>,
    pub now: Now,
    pub ux: Ux,
    pub log: Log,
}

pub struct RelayServer {
    inner: Arc<Inner>,
    local_addr: SocketAddr,
}

struct Inner {
    port: u16,
    scheme: String,
    header_deadline: Duration,
    idle_timeout: Duration,
    manifest_deadline: Duration,
    verbose: bool,
    tls: Option<TlsAcceptor>,
    deps: RelayDeps,
    targets: TargetResolver,
    in_flight: AtomicUsize,
    req_counter: AtomicU64,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}

impl RelayServer {
    pub async fn start(cfg: RelayConfig, deps: RelayDeps) -> io::Result<RelayServer> {
        let scheme = if trust_gateway::is_allowed_scheme(&cfg.scheme) {
            cfg.scheme.to_ascii_lowercase()
        } else {
            "http".to_owned()
        };
        let tls = match (scheme.as_str(), cfg.tls) {
            ("https", Some(config)) => Some(TlsAcceptor::from(config)),
            ("https", None) => {
                let msg = format!(
                    "listener failed on port {} scheme={scheme}: no certificate loaded",
                    cfg.port
                );
                deps.ux.error(Component::Relay, &msg);
                return Err(io::Error::new(io::ErrorKind::InvalidInput, msg));
            }
            _ => None,
        };
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, cfg.port)).await {
            Ok(l) => l,
            Err(e) => {
                deps.ux.error(
                    Component::Relay,
                    &format!("listener failed on port {} scheme={scheme}: {e}", cfg.port),
                );
                return Err(e);
            }
        };
        let local_addr = listener.local_addr()?;
        let timeout = cfg.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let inner = Arc::new(Inner {
            port: local_addr.port(),
            scheme,
            header_deadline: timeout,
            idle_timeout: timeout,
            manifest_deadline: timeout,
            verbose: cfg.verbose,
            tls,
            deps,
            targets: TargetResolver::new(),
            in_flight: AtomicUsize::new(0),
            req_counter: AtomicU64::new(0),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
        });
        let accept = std::sync::Arc::clone(&inner);
        inner.tasks.spawn(accept_loop(accept, listener));
        Ok(RelayServer { inner, local_addr })
    }

    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn scheme(&self) -> &str {
        &self.inner.scheme
    }

    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::SeqCst)
    }

    pub fn remembered_bases(&self) -> usize {
        self.inner.targets.remembered()
    }

    pub async fn stop(&self) {
        self.inner.shutdown.cancel();
        self.inner.tasks.close();
        self.inner.tasks.wait().await;
    }
}

impl Drop for RelayServer {
    fn drop(&mut self) {
        self.inner.shutdown.cancel();
    }
}

async fn accept_loop(inner: Arc<Inner>, listener: TcpListener) {
    loop {
        let accepted = tokio::select! {
            () = inner.shutdown.cancelled() => return,
            a = listener.accept() => a,
        };
        match accepted {
            Ok((stream, _)) => {
                let _ = stream.set_nodelay(true);
                let conn = Arc::clone(&inner);
                inner.tasks.spawn(async move {
                    serve_connection(conn, stream).await;
                });
            }
            Err(e) => {
                inner
                    .deps
                    .ux
                    .warn(Component::Relay, &format!("accept failed: {e}"));
                tokio::select! {
                    () = inner.shutdown.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
        }
    }
}

async fn serve_connection(inner: Arc<Inner>, stream: TcpStream) {
    let abort = inner.shutdown.child_token();
    match inner.tls.clone() {
        Some(acceptor) => {
            let handshake = tokio::select! {
                () = abort.cancelled() => return,
                r = tokio::time::timeout(inner.header_deadline, acceptor.accept(stream)) => r,
            };
            match handshake {
                Ok(Ok(tls)) => drive(inner, TokioIo::new(tls), abort).await,
                Ok(Err(e)) => inner.diag(&format!("tls handshake failed: {e}")),
                Err(_) => inner.diag("tls handshake timed out"),
            }
        }
        None => drive(inner, TokioIo::new(stream), abort).await,
    }
}

async fn drive<I>(inner: Arc<Inner>, io: I, abort: CancellationToken)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let svc_inner = std::sync::Arc::clone(&inner);
    let svc_abort = abort.clone();
    let service = service_fn(move |req: Request<Incoming>| {
        let inner = std::sync::Arc::clone(&svc_inner);
        let abort = svc_abort.clone();
        async move { Ok::<_, Infallible>(handle(inner, req, abort).await) }
    });
    let conn = http1::Builder::new()
        .keep_alive(true)
        .serve_connection(io, service);
    tokio::select! {
        () = abort.cancelled() => {}
        r = conn => {
            if let Err(e) = r {
                inner.diag(&format!("connection ended: {e}"));
            }
        }
    }
}

struct InFlight(Arc<Inner>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Exchange {
    method: Method,
    path: String,
    target: String,
    upstream_class: bool,
    started: Instant,
    header_ms: i64,
    status: u16,
    lazy_state: Option<String>,
    lazy_wait_ms: i64,
    lazy_generator: Option<String>,
    req_id: String,
}

impl Exchange {
    fn sample(&self, bytes_out: u64, failure: Option<&str>) -> TimingSample {
        TimingSample {
            method: self.method.as_str().to_owned(),
            local_path: self.path.clone(),
            target_url: self.target.clone(),
            status: self.status,
            header_ms: self.header_ms,
            total_ms: ms(self.started.elapsed()),
            bytes_out,
            lazy_state: self.lazy_state.clone(),
            lazy_wait_ms: self.lazy_wait_ms,
            lazy_generator: self.lazy_generator.clone(),
            failure: failure.map(str::to_owned),
        }
    }
}

async fn handle(
    inner: Arc<Inner>,
    req: Request<Incoming>,
    abort: CancellationToken,
) -> Response<Body> {
    let path = req.uri().path().to_owned();
    inner.deps.activity.record(&path);
    if inner.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
        inner.in_flight.fetch_sub(1, Ordering::SeqCst);
        return status_only(StatusCode::SERVICE_UNAVAILABLE);
    }
    let guard = InFlight(std::sync::Arc::clone(&inner));
    let req_id = if inner.verbose {
        format!(
            "{:06x}",
            inner.req_counter.fetch_add(1, Ordering::Relaxed) + 1
        )
    } else {
        String::new()
    };
    let method = req.method().clone();
    if inner.verbose {
        inner.diag(&format!(
            "req={req_id} <- {method} {} {:?}",
            short_url(&req.uri().to_string()),
            req.version()
        ));
    }
    if method != Method::GET && method != Method::HEAD {
        inner.diag_verbose(&req_id, "-> 405 (method)");
        let mut resp = status_only(StatusCode::METHOD_NOT_ALLOWED);
        resp.headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
        return resp;
    }
    if req.headers().contains_key(ORIGIN) {
        inner.diag_verbose(&req_id, "-> 403 (origin header)");
        return status_only(StatusCode::FORBIDDEN);
    }
    let host = req.headers().get(HOST).and_then(|h| h.to_str().ok());
    if !security::is_allowed_host_header(host, inner.port) {
        inner.diag_verbose(&req_id, "-> 403 (host)");
        return status_only(StatusCode::FORBIDDEN);
    }
    if !targets::is_play_path(&path) {
        inner.diag_verbose(&req_id, "-> 404 (path mismatch)");
        return status_only(StatusCode::NOT_FOUND);
    }
    let policy = inner
        .deps
        .policy
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let raw_query = req.uri().query().unwrap_or_default();
    let query = if raw_query.is_empty() {
        String::new()
    } else {
        format!("?{raw_query}")
    };
    let target_param = query_value(raw_query, "target");
    let Some(target) = inner
        .targets
        .resolve(&policy, &path, &query, target_param.as_deref())
    else {
        inner.diag_verbose(&req_id, "-> 400 (no target param or relative base)");
        return status_only(StatusCode::BAD_REQUEST);
    };
    let target_url = match security::check_target_url(&policy, &target.url) {
        Ok(u) => u,
        Err(reason) => {
            inner.diag_verbose(
                &req_id,
                &format!("-> 403 ({reason} target='{}')", short_url(&target.url)),
            );
            return status_only(StatusCode::FORBIDDEN);
        }
    };
    inner.diag_verbose(
        &req_id,
        &format!(
            "kind={} target={}",
            target.kind.as_str(),
            short_url(&target.url)
        ),
    );
    let upstream_class = target_url
        .host_str()
        .is_some_and(|h| policy.is_first_party_host(h));
    inner.deps.stats.record_relay_request(upstream_class);

    let mut ex = Exchange {
        method: method.clone(),
        path,
        target: target.url.clone(),
        upstream_class,
        started: Instant::now(),
        header_ms: -1,
        status: 0,
        lazy_state: None,
        lazy_wait_ms: -1,
        lazy_generator: None,
        req_id,
    };
    let outbound = UpstreamRequest {
        method: method.clone(),
        url: target_url,
        headers: headers::forward_request_headers(req.headers()),
    };
    let sent = tokio::select! {
        () = inner.shutdown.cancelled() => return status_only(StatusCode::SERVICE_UNAVAILABLE),
        r = tokio::time::timeout(inner.header_deadline, inner.deps.upstream.send(outbound)) => r,
    };
    let resp = match sent {
        Err(_) => {
            inner.deps.ux.warn(
                Component::Relay,
                &format!("upstream timed out for {}", short_url(&ex.target)),
            );
            inner.record_hitch(&ex.sample(0, Some("upstream_timeout")));
            return status_only(StatusCode::GATEWAY_TIMEOUT);
        }
        Ok(Err(e)) => {
            inner.deps.ux.warn(
                Component::Relay,
                &format!(
                    "request failed for {}: {}: {}",
                    short_url(&ex.target),
                    e.kind,
                    e.message
                ),
            );
            inner.record_hitch(&ex.sample(0, Some(e.kind)));
            return status_only(StatusCode::BAD_GATEWAY);
        }
        Ok(Ok(r)) => r,
    };
    ex.header_ms = ms(ex.started.elapsed());
    ex.status = resp.status.as_u16();
    ex.lazy_state = headers::first_header(&resp.headers, "x-lazy-hls");
    ex.lazy_wait_ms = headers::long_header(&resp.headers, "x-lazy-hls-wait-ms");
    ex.lazy_generator = headers::first_header(&resp.headers, "x-lazy-hls-generator");
    inner.diag_verbose(
        &ex.req_id,
        &format!(
            "upstream-status={} ms={}",
            ex.status,
            ms(ex.started.elapsed())
        ),
    );

    let is_head = method == Method::HEAD;
    let content_type = headers::first_header(&resp.headers, "content-type");
    if !is_head
        && resp.status.is_success()
        && manifest::is_likely_manifest(&ex.path, &ex.target, content_type.as_deref())
    {
        return respond_manifest(&inner, ex, resp, policy, content_type, guard, abort);
    }
    respond_stream(&inner, ex, resp, content_type, is_head, guard, abort)
}

struct ChannelSink {
    tx: mpsc::Sender<Bytes>,
    idle: Duration,
}

impl ManifestSink for ChannelSink {
    async fn write(&mut self, chunk: Bytes) -> io::Result<()> {
        match tokio::time::timeout(self.idle, self.tx.send(chunk)).await {
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "client write stalled",
            )),
            Ok(Err(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client went away",
            )),
            Ok(Ok(())) => Ok(()),
        }
    }
}

fn status_only(status: StatusCode) -> Response<Body> {
    let mut resp = Response::new(Empty::new().map_err(never).boxed());
    *resp.status_mut() = status;
    resp
}

fn never(n: Infallible) -> io::Error {
    match n {}
}

fn ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

pub fn query_value(raw_query: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(raw_query.as_bytes())
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.into_owned())
}

impl Inner {
    fn record_hitch(&self, sample: &TimingSample) {
        if sample.target_url.trim().is_empty() {
            return;
        }
        let perf = &self.deps.stats.perf;
        if perf.enabled() && sample.method.eq_ignore_ascii_case("GET") {
            let failed = sample.status >= 400
                || sample
                    .failure
                    .as_deref()
                    .is_some_and(|f| !f.trim().is_empty());
            perf.record_relay(
                hitch::relay_kind(sample),
                failed,
                sample.header_ms,
                sample.total_ms,
                sample.bytes_out,
            );
        }
        self.deps
            .hitch
            .record(sample, (self.deps.now)(), &self.deps.ux, &self.deps.log);
    }

    fn diag(&self, message: &str) {
        if !self.verbose {
            return;
        }
        if self.deps.log.dev_console() {
            self.deps.ux.write(Component::Relay, message);
        } else {
            self.deps.log.line(&format!("[relay] {message}"));
        }
    }

    fn diag_verbose(&self, req_id: &str, message: &str) {
        if self.verbose {
            self.diag(&format!("req={req_id} {message}"));
        }
    }
}

pub fn play_url(port: u16, scheme: &str, play_id: &str, file: &str, target: &str) -> String {
    format!(
        "{scheme}://127.0.0.1:{port}/play/{play_id}/{file}?target={}",
        targets::encode_target_param(target)
    )
}

mod streaming;
use streaming::*;
