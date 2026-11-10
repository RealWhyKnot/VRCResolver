use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::task::Poll;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use http::header::{ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderValue, ORIGIN};
use http::{Method, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, StreamBody};
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
use super::targets::{self, TargetResolver};
use super::upstream::{BodyStream, Upstream, UpstreamRequest, UpstreamResponse};
use crate::clock::Now;
use crate::local::store::Manifests;
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
    pub recovery: super::retry::Hooks,
    pub upstream: Arc<dyn Upstream>,
    pub policy: Arc<RwLock<FirstPartyPolicy>>,
    pub activity: Arc<RelayActivity>,
    pub hitch: Arc<HitchDetector>,
    pub stats: Arc<Stats>,
    pub manifests: Arc<Manifests>,
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
    live_playlists: super::live::history::History,
    retries: super::retry::RetryMemory,
    in_flight: AtomicUsize,
    req_counter: AtomicU64,
    request_prefix: String,
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
            live_playlists: super::live::history::History::default(),
            retries: super::retry::RetryMemory::default(),
            in_flight: AtomicUsize::new(0),
            req_counter: AtomicU64::new(0),
            request_prefix: uuid::Uuid::new_v4().simple().to_string(),
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
        self.inner.report_retries(true);
        self.inner
            .deps
            .stats
            .playback
            .close_all((self.inner.deps.now)(), &self.inner.deps.ux);
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
                inner.connection_failure(
                    &inner.next_id("accept"),
                    Instant::now(),
                    &format!("accept_{:?}", e.kind()),
                );
                tokio::select! {
                    () = inner.shutdown.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
        }
    }
}

async fn serve_connection(inner: Arc<Inner>, stream: TcpStream) {
    let started = Instant::now();
    let connection_id = inner.next_id("connection");
    let abort = inner.shutdown.child_token();
    match inner.tls.clone() {
        Some(acceptor) => {
            let handshake = tokio::select! {
                () = abort.cancelled() => return,
                r = tokio::time::timeout(inner.header_deadline, acceptor.accept(stream)) => r,
            };
            match handshake {
                Ok(Ok(tls)) => drive(inner, TokioIo::new(tls), abort).await,
                Ok(Err(_)) => {
                    inner.connection_failure(&connection_id, started, "tls_handshake_failed")
                }
                Err(_) => {
                    inner.connection_failure(&connection_id, started, "tls_handshake_timeout")
                }
            }
        }
        None => drive(inner, TokioIo::new(stream), abort).await,
    }
}

async fn drive<I>(inner: Arc<Inner>, io: I, abort: CancellationToken)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let started = Instant::now();
    let connection_id = inner.next_id("http");
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
            if let Err(error) = r && error.is_parse() { inner.connection_failure(&connection_id, started, "http_parse_failed"); }
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
    suppressed: bool,
    recovery: bool,
    retry_reason: Option<String>,
    req_id: String,
    stream_id: String,
    player_status: u16,
}

impl Exchange {
    fn sample(&self, bytes_out: u64, failure: Option<&str>) -> TimingSample {
        TimingSample {
            suppressed: self.suppressed,
            recovery: self.recovery,
            retry_reason: self.retry_reason.clone(),
            request_id: self.req_id.clone(),
            stream_id: self.stream_id.clone(),
            player_status: self.player_status,
            playlist_shape: None,
            method: self.method.as_str().to_owned(),
            local_path: self.path.clone(),
            target_url: self.target.clone(),
            status: self.status,
            header_ms: self.header_ms,
            total_ms: ms(self.started.elapsed()),
            bytes_out,
            media: None,
            lazy_state: self.lazy_state.clone(),
            lazy_wait_ms: self.lazy_wait_ms,
            lazy_generator: self.lazy_generator.clone(),
            failure: failure.map(str::to_owned),
        }
    }
}

struct HeaderAttempt {
    inner: Arc<Inner>,
    sample: TimingSample,
    started: Instant,
    complete: bool,
}

impl Drop for HeaderAttempt {
    fn drop(&mut self) {
        if !self.complete {
            self.sample.total_ms = ms(self.started.elapsed());
            self.inner.record_hitch(&self.sample);
        }
    }
}

async fn handle(
    inner: Arc<Inner>,
    req: Request<Incoming>,
    abort: CancellationToken,
) -> Response<Body> {
    let started = Instant::now();
    let req_id = inner.next_id("relay");
    let path = req.uri().path().to_owned();
    inner.deps.activity.record(&path);
    if inner.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
        inner.in_flight.fetch_sub(1, Ordering::SeqCst);
        return inner.reject(
            &req,
            &req_id,
            StatusCode::SERVICE_UNAVAILABLE,
            "connection_limit",
            started,
        );
    }
    let guard = InFlight(std::sync::Arc::clone(&inner));
    let method = req.method().clone();
    if method != Method::GET && method != Method::HEAD {
        let mut resp = inner.reject(
            &req,
            &req_id,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_refused",
            started,
        );
        resp.headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
        return resp;
    }
    if req.headers().contains_key(ORIGIN) {
        return inner.reject(
            &req,
            &req_id,
            StatusCode::FORBIDDEN,
            "origin_refused",
            started,
        );
    }
    let host = req.headers().get(HOST).and_then(|h| h.to_str().ok());
    if !security::is_allowed_host_header(host, inner.port) {
        return inner.reject(
            &req,
            &req_id,
            StatusCode::FORBIDDEN,
            "host_refused",
            started,
        );
    }
    if !targets::is_play_path(&path) || super::activity::play_id(&path).is_none() {
        return inner.reject(
            &req,
            &req_id,
            StatusCode::NOT_FOUND,
            "route_missing",
            started,
        );
    }
    if let Some(token) = crate::local::token(&path) {
        let manifest = inner.deps.manifests.get(token);

        return match manifest {
            Some(body) => {
                let size = u64::try_from(body.len()).unwrap_or(u64::MAX);
                let shape = manifest::LocalizeResult {
                    bytes_in: size,
                    bytes_out: size,
                    kind: manifest::PlaylistKind::Master,
                    ..Default::default()
                }
                .shape();
                inner.record_hitch(&TimingSample {
                    suppressed: false,
                    recovery: false,
                    retry_reason: None,
                    request_id: req_id,
                    stream_id: String::new(),
                    player_status: 200,
                    playlist_shape: Some(shape),
                    method: method.to_string(),
                    local_path: path.clone(),
                    target_url: String::new(),
                    status: 200,
                    header_ms: ms(started.elapsed()),
                    total_ms: ms(started.elapsed()),
                    bytes_out: if method == Method::HEAD { 0 } else { size },
                    media: None,
                    lazy_state: None,
                    lazy_wait_ms: -1,
                    lazy_generator: None,
                    failure: None,
                });
                local_manifest(body, method == Method::HEAD)
            }
            None => inner.reject(
                &req,
                &req_id,
                StatusCode::NOT_FOUND,
                "local_manifest_gone",
                started,
            ),
        };
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
        return inner.reject(
            &req,
            &req_id,
            StatusCode::BAD_REQUEST,
            "target_missing",
            started,
        );
    };
    let target_url = match security::check_target_url(&policy, &target.url) {
        Ok(u) => u,
        Err(reason) => {
            return inner.reject(&req, &req_id, StatusCode::FORBIDDEN, reason, started);
        }
    };

    let upstream_class = target_url
        .host_str()
        .is_some_and(|h| policy.is_first_party_host(h));
    inner.deps.stats.record_relay_request(upstream_class);

    let mut ex = Exchange {
        method: method.clone(),
        path,
        target: target.url.clone(),
        upstream_class,
        started,
        header_ms: -1,
        status: 0,
        lazy_state: None,
        lazy_wait_ms: -1,
        lazy_generator: None,
        suppressed: false,
        recovery: false,
        retry_reason: None,
        req_id,
        stream_id: crate::diagnostics::stream_id(&target.url),
        player_status: 0,
    };
    inner.deps.stats.playback.begin_request(
        &ex.path,
        &ex.target,
        &ex.req_id,
        (inner.deps.now)(),
        &inner.deps.ux,
    );
    let mut outbound = UpstreamRequest {
        method: method.clone(),
        url: target_url,
        headers: headers::forward_request_headers(req.headers()),
    };
    let eligible = matches!(
        hitch::relay_kind(&ex.sample(0, None)),
        crate::stats::RelayKind::Manifest | crate::stats::RelayKind::Segment
    );
    let playback = super::activity::play_id(&ex.path)
        .unwrap_or_default()
        .to_owned();
    let key = super::retry::key(&playback, &ex.target, &method, &outbound.headers);
    outbound.headers.insert(
        "x-request-id",
        HeaderValue::from_str(&ex.req_id).expect("generated request id"),
    );
    let mut attempt = HeaderAttempt {
        inner: Arc::clone(&inner),
        sample: ex.sample(0, Some("client_disconnect")),
        started: ex.started,
        complete: false,
    };
    let now = (inner.deps.now)();
    let retired = eligible && inner.retries.is_retired(&playback, now);
    let cached = eligible.then(|| inner.retries.get(&key, now)).flatten();
    let mut resp = if retired {
        ex.suppressed = true;
        ex.retry_reason = Some(super::retry::RECREATE.to_owned());
        retry::terminal()
    } else if let Some(answer) = cached {
        ex.suppressed = true;
        ex.retry_reason = Some(answer.reason.to_owned());
        retry::response(answer)
    } else {
        let sent = tokio::select! {
            () = inner.shutdown.cancelled() => { attempt.sample.failure = Some("shutdown".to_owned()); attempt.sample.player_status = 503; return status_only(StatusCode::SERVICE_UNAVAILABLE) },
            r = tokio::time::timeout(inner.header_deadline, inner.deps.upstream.send(outbound)) => r,
        };
        match sent {
            Err(_) => {
                attempt.complete = true;
                ex.player_status = 504;
                inner.record_hitch(&ex.sample(0, Some("upstream_timeout")));
                return status_only(StatusCode::GATEWAY_TIMEOUT);
            }
            Ok(Err(e)) => {
                attempt.complete = true;
                ex.player_status = 502;
                inner.record_hitch(&ex.sample(0, Some(e.kind)));
                return status_only(StatusCode::BAD_GATEWAY);
            }
            Ok(Ok(r)) => r,
        }
    };
    if eligible && !ex.suppressed {
        if resp.status.is_success() {
            inner.retries.clear(&key);
        } else if matches!(resp.status.as_u16(), 404 | 410 | 429 | 502 | 503 | 504) {
            ex.status = resp.status.as_u16();
            if let Some(id) = headers::first_header(&resp.headers, "x-stream-id") {
                ex.stream_id = crate::diagnostics::token(&id);
            }
            let trusted = resp.direct
                && inner
                    .deps
                    .stats
                    .playback
                    .trusts_origin(&ex.path, &ex.target);
            let header_reason = if trusted {
                super::retry::reason(&resp.headers, &[])
            } else {
                "upstream_status"
            };
            let (received, body) = if method == Method::HEAD
                || header_reason == super::retry::RECREATE
            {
                (resp, (method == Method::HEAD).then(Bytes::new))
            } else {
                tokio::select! {
                    () = inner.shutdown.cancelled() => return status_only(StatusCode::SERVICE_UNAVAILABLE),
                    inspected = retry::inspect(resp) => inspected,
                }
            };
            resp = received;
            let reason = if trusted {
                super::retry::reason(&resp.headers, body.as_deref().unwrap_or_default())
            } else {
                "upstream_status"
            };
            if reason != "upstream_status" || matches!(resp.status.as_u16(), 429 | 502 | 503 | 504)
            {
                ex.retry_reason = Some(reason.to_owned());
            }
            if reason == super::retry::RECREATE {
                inner.retire(&mut ex, &playback);
                resp = retry::terminal();
            } else if let Some(body) = body {
                let now = (inner.deps.now)();
                if let Some(delay) =
                    super::retry::delay(resp.status.as_u16(), reason, &resp.headers, now)
                {
                    let answer = super::retry::Answer {
                        status: resp.status,
                        headers: resp.headers.clone(),
                        body,
                        reason,
                        until: now + delay,
                    };
                    if inner.retries.confirmed(&key, &answer, now) {
                        inner.retries.put(key, answer, now);
                    }
                }
            }
        }
    }
    attempt.complete = true;
    ex.header_ms = ms(ex.started.elapsed());
    if ex.status == 0 {
        ex.status = resp.status.as_u16();
    }
    ex.player_status = resp.status.as_u16();
    if let Some(id) = headers::first_header(&resp.headers, "x-stream-id") {
        ex.stream_id = crate::diagnostics::token(&id);
    }
    ex.lazy_state = headers::first_header(&resp.headers, "x-lazy-hls");
    ex.lazy_wait_ms = headers::long_header(&resp.headers, "x-lazy-hls-wait-ms");
    ex.lazy_generator = headers::first_header(&resp.headers, "x-lazy-hls-generator");

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
    written: u64,
}

impl ManifestSink for ChannelSink {
    async fn write(&mut self, chunk: Bytes) -> io::Result<()> {
        let size = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        match tokio::time::timeout(self.idle, self.tx.send(chunk)).await {
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "client write stalled",
            )),
            Ok(Err(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client went away",
            )),
            Ok(Ok(())) => {
                self.written += size;
                Ok(())
            }
        }
    }
}

fn local_manifest(manifest: String, is_head: bool) -> Response<Body> {
    let length = HeaderValue::from(manifest.len());
    let body = if is_head {
        Empty::new().map_err(never).boxed()
    } else {
        Full::new(Bytes::from(manifest)).map_err(never).boxed()
    };
    let mut resp = Response::new(body);
    let headers = resp.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(crate::local::MANIFEST_TYPE),
    );
    headers.insert(CONTENT_LENGTH, length);
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
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
    fn next_id(&self, kind: &str) -> String {
        format!(
            "{kind}-{}-{:x}",
            self.request_prefix,
            self.req_counter.fetch_add(1, Ordering::Relaxed) + 1
        )
    }

    fn connection_failure(&self, id: &str, started: Instant, reason: &str) {
        self.record_hitch(&TimingSample {
            suppressed: false,
            recovery: false,
            retry_reason: None,
            request_id: id.to_owned(),
            stream_id: String::new(),
            player_status: 0,
            playlist_shape: None,
            method: "CONNECT".to_owned(),
            local_path: "/play/unassigned/connection".to_owned(),
            target_url: String::new(),
            status: 0,
            header_ms: -1,
            total_ms: ms(started.elapsed()),
            bytes_out: 0,
            media: None,
            lazy_state: None,
            lazy_wait_ms: -1,
            lazy_generator: None,
            failure: Some(reason.to_owned()),
        });
    }

    fn reject(
        &self,
        request: &Request<Incoming>,
        id: &str,
        status: StatusCode,
        reason: &str,
        started: Instant,
    ) -> Response<Body> {
        self.record_hitch(&TimingSample {
            suppressed: false,
            recovery: false,
            retry_reason: None,
            request_id: id.to_owned(),
            stream_id: String::new(),
            player_status: status.as_u16(),
            playlist_shape: None,
            method: request.method().to_string(),
            local_path: request.uri().path().to_owned(),
            target_url: String::new(),
            status: 0,
            header_ms: -1,
            total_ms: ms(started.elapsed()),
            bytes_out: 0,
            media: None,
            lazy_state: None,
            lazy_wait_ms: -1,
            lazy_generator: None,
            failure: Some(reason.to_owned()),
        });
        status_only(status)
    }

    fn record_hitch(&self, sample: &TimingSample) {
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
        let now = (self.deps.now)();
        let diagnostic = self.deps.hitch.analyze(sample, now);
        self.deps.stats.perf.record_diagnostics(
            diagnostic
                .as_ref()
                .map_or("", |diagnostic| diagnostic.reasons.as_str()),
            sample.status,
        );
        self.deps.stats.playback.request(
            sample,
            diagnostic
                .as_ref()
                .map_or("", |diagnostic| diagnostic.reasons.as_str()),
            now,
            &self.deps.ux,
        );
        self.report_retries(false);
        if sample.status < 300 && sample.failure.is_none() {
            self.deps.hitch.record_diagnostic(
                sample,
                now,
                &self.deps.ux,
                &self.deps.log,
                diagnostic,
            );
            if self.verbose {
                self.deps.log.line(&format!("[relay][debug] req={} kind={} host={} status={} elapsed_ms={} bytes={} sent={}", sample.request_id, match hitch::relay_kind(sample) { crate::stats::RelayKind::Manifest => "manifest", crate::stats::RelayKind::Segment => "segment", crate::stats::RelayKind::Other => "other" }, crate::diagnostics::host(&sample.target_url), sample.status, sample.total_ms, sample.bytes_out, sample.player_status));
            }
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

mod retry;
