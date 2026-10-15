use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use http::{HeaderMap, Method, StatusCode};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect;
use url::{Host, Url};
use vrcr_shared::url_guard;

pub const MAX_REDIRECTS: usize = 5;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub type BodyStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;
pub type Sending<'a> = Pin<Box<dyn Future<Output = Result<UpstreamResponse, UpstreamError>> + Send + 'a>>;

pub struct UpstreamRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
}

pub struct UpstreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: BodyStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamError {
    pub kind: &'static str,
    pub message: String,
}

impl UpstreamError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> UpstreamError {
        UpstreamError {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

pub trait Upstream: Send + Sync {
    fn send(&self, req: UpstreamRequest) -> Sending<'_>;
}

pub fn blocked_message(addr: std::net::IpAddr) -> String {
    format!("Refused connection to a blocked internal address ({addr}).")
}

pub fn blocked_literal(url: &Url) -> Option<std::net::IpAddr> {
    let ip = match url.host()? {
        Host::Ipv4(v4) => std::net::IpAddr::V4(v4),
        Host::Ipv6(v6) => std::net::IpAddr::V6(v6),
        Host::Domain(_) => return None,
    };
    url_guard::is_blocked(ip).then_some(ip)
}

pub struct ReqwestUpstream {
    client: reqwest::Client,
}

impl ReqwestUpstream {
    pub fn new(tls: rustls::ClientConfig) -> Result<ReqwestUpstream, String> {
        ReqwestUpstream::build(tls, &[])
    }

    pub fn with_overrides(
        tls: rustls::ClientConfig,
        overrides: &[(&str, SocketAddr)],
    ) -> Result<ReqwestUpstream, String> {
        ReqwestUpstream::build(tls, overrides)
    }

    fn build(tls: rustls::ClientConfig, overrides: &[(&str, SocketAddr)]) -> Result<ReqwestUpstream, String> {
        let policy = redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            match blocked_literal(attempt.url()) {
                Some(ip) => attempt.error(blocked_message(ip)),
                None => attempt.follow(),
            }
        });
        let mut builder = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .dns_resolver(GuardedResolver)
            .redirect(policy)
            .referer(false)
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .tcp_nodelay(true);
        for (host, addr) in overrides {
            builder = builder.resolve(host, *addr);
        }
        let client = builder.build().map_err(|e| e.to_string())?;
        Ok(ReqwestUpstream { client })
    }
}

impl Upstream for ReqwestUpstream {
    fn send(&self, req: UpstreamRequest) -> Sending<'_> {
        Box::pin(async move {
            if let Some(ip) = blocked_literal(&req.url) {
                return Err(UpstreamError::new("blocked_address", blocked_message(ip)));
            }
            let resp = self
                .client
                .request(req.method, req.url)
                .headers(req.headers)
                .send()
                .await
                .map_err(describe)?;
            let status = resp.status();
            let headers = resp.headers().clone();
            let body: BodyStream = Box::pin(resp.bytes_stream().map_err(io::Error::other));
            Ok(UpstreamResponse {
                status,
                headers,
                body,
            })
        })
    }
}

fn describe(e: reqwest::Error) -> UpstreamError {
    let e = e.without_url();
    let mut message = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        message.push_str(": ");
        message.push_str(&s.to_string());
        source = s.source();
    }
    let kind = if message.contains("blocked internal address") {
        "blocked_address"
    } else if e.is_timeout() {
        "timeout"
    } else if e.is_redirect() {
        "redirect_error"
    } else if e.is_connect() {
        "connect_error"
    } else {
        "request_error"
    };
    UpstreamError::new(kind, message)
}

struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if let Some(bad) = addrs.iter().find(|a| url_guard::is_blocked(a.ip())) {
                let err = io::Error::new(io::ErrorKind::PermissionDenied, blocked_message(bad.ip()));
                return Err(Box::new(err) as Box<dyn std::error::Error + Send + Sync>);
            }
            let addrs: Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

pub fn empty_body() -> BodyStream {
    Box::pin(futures_util::stream::empty())
}

pub fn body_from_chunks(chunks: Vec<Bytes>) -> BodyStream {
    Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok)))
}

pub fn shared(upstream: impl Upstream + 'static) -> Arc<dyn Upstream> {
    Arc::new(upstream)
}

pub async fn collect(mut body: BodyStream) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        out.extend_from_slice(&chunk?);
    }
    Ok(out)
}
