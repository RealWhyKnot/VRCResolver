use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

pub fn is_listening(port: u16, timeout: Duration) -> bool {
    port != 0 && TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout).is_ok()
}

pub fn terminal_failure_line(
    request_id: &str,
    domain: &str,
    player: &str,
    server_reason: &str,
    bundled_reason: &str,
    elapsed_ms: i64,
) -> String {
    format!(
        "terminal_failure request_id={request_id} domain={domain} player={player} server_reason={server_reason} bundled_ytdlp_reason={bundled_reason} elapsed_ms={elapsed_ms}"
    )
}
