use std::io;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
pub const NOTIFY_TIMEOUT: Duration = Duration::from_millis(1500);
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const CONNECT_POLL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub enum ConnectError {
    TimedOut,
    NotFound,
    Other(io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => f.write_str("timed out"),
            Self::NotFound => f.write_str("not found"),
            Self::Other(e) => write!(f, "{:?}: {e}", e.kind()),
        }
    }
}

pub fn full_name(pipe_name: &str) -> String {
    format!(r"\\.\pipe\{pipe_name}")
}

#[cfg(windows)]
pub type PipeStream = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(windows)]
pub async fn connect(pipe_name: &str, timeout: Duration) -> Result<PipeStream, ConnectError> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    let name = full_name(pipe_name);
    let deadline = Instant::now() + timeout;
    loop {
        match ClientOptions::new().open(&name) {
            Ok(client) => return Ok(client),
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {}
            Err(e) => return Err(ConnectError::Other(e)),
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(ConnectError::TimedOut);
        }
        tokio::time::sleep(CONNECT_POLL.min(deadline - now)).await;
    }
}

#[cfg(not(windows))]
pub type PipeStream = tokio::net::UnixStream;

#[cfg(not(windows))]
pub async fn connect(pipe_name: &str, timeout: Duration) -> Result<PipeStream, ConnectError> {
    let path = std::env::temp_dir().join(pipe_name);
    let deadline = Instant::now() + timeout;
    loop {
        match tokio::net::UnixStream::connect(&path).await {
            Ok(s) => return Ok(s),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) => {}
            Err(e) => return Err(ConnectError::Other(e)),
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(ConnectError::TimedOut);
        }
        tokio::time::sleep(CONNECT_POLL.min(deadline - now)).await;
    }
}

pub async fn read_line<R: AsyncRead + Unpin>(stream: &mut R) -> io::Result<Option<String>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    while out.len() < MAX_RESPONSE_BYTES {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let newline = buf[..n].iter().position(|b| *b == b'\n');
        let take = newline.unwrap_or(n);
        for &b in &buf[..take] {
            if out.len() >= MAX_RESPONSE_BYTES {
                break;
            }
            if b != b'\r' {
                out.push(b);
            }
        }
        if newline.is_some() {
            break;
        }
    }
    Ok((!out.is_empty()).then(|| String::from_utf8_lossy(&out).into_owned()))
}

pub async fn send_frame<W: AsyncWrite + Unpin>(stream: &mut W, frame: &[u8]) -> io::Result<()> {
    stream.write_all(frame).await?;
    stream.flush().await
}
