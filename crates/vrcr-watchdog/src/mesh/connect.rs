use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use url::{Host, Url};

use vrcr_shared::wire::SUBPROTOCOL_V3;

pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESPONSE_HEAD_BYTES: usize = 16 * 1024;

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub type Socket = WebSocketStream<Box<dyn Io>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Dns,
    Tcp,
    Upgrade,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Tcp => "tcp",
            Self::Upgrade => "upgrade",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Probe {
    pub phase: Phase,
    pub dns_ms: Option<u64>,
    pub tcp_ms: Option<u64>,
}

impl Probe {
    pub fn timings(&self) -> String {
        let mut parts = Vec::with_capacity(2);
        if let Some(ms) = self.dns_ms {
            parts.push(format!("dns {ms} ms"));
        }
        if let Some(ms) = self.tcp_ms {
            parts.push(format!("tcp {ms} ms"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        }
    }
}

#[derive(Debug)]
pub enum ConnectError {
    Url(String),
    Dns(io::Error),
    Tcp(io::Error),
    Tls(io::Error),
    Handshake(String),
}

impl ConnectError {
    pub fn describe(&self) -> String {
        match self {
            Self::Url(m) => format!("UrlError: {m}"),
            Self::Dns(e) => format!("DnsError: {e}"),
            Self::Tcp(e) => format!("TcpError: {e}"),
            Self::Tls(e) => format!("TlsError: {e}"),
            Self::Handshake(m) => format!("HandshakeError: {m}"),
        }
    }
}

pub struct Opened {
    pub socket: Socket,
    pub subprotocol: Option<String>,
}

pub fn platform_tls() -> Result<Arc<ClientConfig>, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_platform_verifier()
        .map_err(|e| e.to_string())?
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn ms_since(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub async fn open(
    url: &str,
    user_agent: &str,
    tls: impl FnOnce() -> Result<Arc<ClientConfig>, String>,
    probe: &mut Probe,
) -> Result<Opened, ConnectError> {
    let parsed = Url::parse(url).map_err(|e| ConnectError::Url(e.to_string()))?;
    let secure = match parsed.scheme() {
        "wss" => true,
        "ws" => false,
        other => return Err(ConnectError::Url(format!("unsupported scheme {other}"))),
    };
    let host = match parsed.host() {
        Some(Host::Domain(d)) => d.to_owned(),
        Some(Host::Ipv4(a)) => a.to_string(),
        Some(Host::Ipv6(a)) => a.to_string(),
        None => return Err(ConnectError::Url("missing host".to_owned())),
    };
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if secure { 443 } else { 80 });

    probe.phase = Phase::Dns;
    let started = Instant::now();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(ConnectError::Dns)?
        .collect();
    probe.dns_ms = Some(ms_since(started));
    if addrs.is_empty() {
        return Err(ConnectError::Dns(io::Error::new(
            io::ErrorKind::NotFound,
            "no addresses for host",
        )));
    }

    probe.phase = Phase::Tcp;
    let started = Instant::now();
    let tcp = connect_any(&addrs).await.map_err(ConnectError::Tcp)?;
    let _ = tcp.set_nodelay(true);
    probe.tcp_ms = Some(ms_since(started));

    probe.phase = Phase::Upgrade;
    let stream: Box<dyn Io> = if secure {
        let config = tls().map_err(|e| ConnectError::Tls(io::Error::other(e)))?;
        let name = ServerName::try_from(host.clone())
            .map_err(|e| ConnectError::Tls(io::Error::new(io::ErrorKind::InvalidInput, e)))?;
        let tls_stream = TlsConnector::from(config)
            .connect(name, tcp)
            .await
            .map_err(ConnectError::Tls)?;
        Box::new(tls_stream)
    } else {
        Box::new(tcp)
    };

    let mut target = parsed.path().to_owned();
    if let Some(q) = parsed.query() {
        target.push('?');
        target.push_str(q);
    }
    let host_header = match parsed.port() {
        Some(p) => format!("{}:{p}", parsed.host_str().unwrap_or_default()),
        None => parsed.host_str().unwrap_or_default().to_owned(),
    };
    let (stream, leftover, subprotocol) = handshake(stream, &target, &host_header, user_agent).await?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let socket = WebSocketStream::from_partially_read(stream, leftover, Role::Client, Some(config)).await;
    Ok(Opened { socket, subprotocol })
}

async fn connect_any(addrs: &[SocketAddr]) -> io::Result<TcpStream> {
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no addresses for host")))
}

pub fn upgrade_request(target: &str, host: &str, user_agent: &str, key: &str) -> String {
    format!(
        "GET {target} HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Protocol: {SUBPROTOCOL_V3}\r\nUser-Agent: {user_agent}\r\n\r\n"
    )
}

async fn handshake(
    mut stream: Box<dyn Io>,
    target: &str,
    host: &str,
    user_agent: &str,
) -> Result<(Box<dyn Io>, Vec<u8>, Option<String>), ConnectError> {
    let key = generate_key();
    let request = upgrade_request(target, host, user_agent, &key);
    let io_err = |e: io::Error| ConnectError::Handshake(e.to_string());
    stream.write_all(request.as_bytes()).await.map_err(io_err)?;
    stream.flush().await.map_err(io_err)?;

    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(end) = find_head_end(&buf) {
            break end;
        }
        if buf.len() > MAX_RESPONSE_HEAD_BYTES {
            return Err(ConnectError::Handshake(
                "upgrade response headers too large".to_owned(),
            ));
        }
        let n = stream.read(&mut chunk).await.map_err(io_err)?;
        if n == 0 {
            return Err(ConnectError::Handshake(
                "connection closed during the upgrade".to_owned(),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let leftover = buf.split_off(head_end);
    let head = String::from_utf8_lossy(&buf).into_owned();
    let subprotocol = check_upgrade_response(&head, &key).map_err(ConnectError::Handshake)?;
    Ok((stream, leftover, subprotocol))
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

pub fn check_upgrade_response(head: &str, key: &str) -> Result<Option<String>, String> {
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if code != "101" {
        return Err(format!("server returned HTTP {code} instead of 101"));
    }
    let mut upgrade = None;
    let mut connection = None;
    let mut accept = None;
    let mut protocol = None;
    let mut extensions = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_owned();
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" => upgrade = Some(value),
            "connection" => connection = Some(value),
            "sec-websocket-accept" => accept = Some(value),
            "sec-websocket-protocol" => protocol = Some(value),
            "sec-websocket-extensions" => extensions = Some(value),
            _ => {}
        }
    }
    if !upgrade.is_some_and(|u| u.eq_ignore_ascii_case("websocket")) {
        return Err("upgrade response is missing Upgrade: websocket".to_owned());
    }
    if !connection.is_some_and(|c| c.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade"))) {
        return Err("upgrade response is missing Connection: Upgrade".to_owned());
    }
    if accept.as_deref() != Some(derive_accept_key(key.as_bytes()).as_str()) {
        return Err("upgrade response carries the wrong Sec-WebSocket-Accept".to_owned());
    }
    if extensions.is_some_and(|e| !e.is_empty()) {
        return Err("server enabled a websocket extension that was not offered".to_owned());
    }
    match protocol.filter(|p| !p.is_empty()) {
        None => Ok(None),
        Some(p) if p == SUBPROTOCOL_V3 => Ok(Some(p)),
        Some(p) => Err(format!("server picked a subprotocol that was not offered: {p}")),
    }
}
