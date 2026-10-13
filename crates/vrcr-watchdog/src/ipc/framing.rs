use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use url::Url;
use vrcr_shared::logutil;
use vrcr_shared::wire::{Player, ResolveRequest, ResolveResponse, action};

pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
pub const MARKER_HOST: &str = "localhost.youtube.com";
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: Option<String>,
    pub truncated: bool,
}

pub async fn read_line<S: AsyncRead + Unpin>(s: &mut S) -> std::io::Result<Line> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    let mut saw_newline = false;
    while out.len() < MAX_REQUEST_BYTES {
        let n = s.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        let consume = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => {
                saw_newline = true;
                i
            }
            None => n,
        };
        for b in &chunk[..consume] {
            if out.len() >= MAX_REQUEST_BYTES {
                break;
            }
            if *b != b'\r' {
                out.push(*b);
            }
        }
        if saw_newline {
            break;
        }
    }
    if out.is_empty() {
        return Ok(Line {
            text: None,
            truncated: false,
        });
    }
    Ok(Line {
        truncated: !saw_newline && out.len() >= MAX_REQUEST_BYTES,
        text: Some(String::from_utf8_lossy(&out).into_owned()),
    })
}

pub async fn write_frame<S: AsyncWrite + Unpin>(s: &mut S, frame: &[u8]) -> std::io::Result<()> {
    let mut payload = Vec::with_capacity(frame.len() + 1);
    payload.extend_from_slice(frame);
    payload.push(b'\n');
    match tokio::time::timeout(WRITE_TIMEOUT, async {
        s.write_all(&payload).await?;
        s.flush().await
    })
    .await
    {
        Ok(r) => r,
        Err(_) => Err(std::io::ErrorKind::TimedOut.into()),
    }
}

pub fn fallback_frame(id: &str, reason: &str) -> Vec<u8> {
    let frame = ResolveResponse {
        action: action::FALLBACK_NATIVE.to_owned(),
        id: id.to_owned(),
        reason: Some(reason.to_owned()),
        ..ResolveResponse::default()
    };
    serde_json::to_vec(&frame).unwrap_or_default()
}

pub async fn write_fallback<S: AsyncWrite + Unpin>(s: &mut S, id: &str, reason: &str) {
    let _ = write_frame(s, &fallback_frame(id, reason)).await;
}

pub fn is_localhost_youtube_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.eq_ignore_ascii_case(MARKER_HOST)))
        .unwrap_or(false)
}

pub fn player_label(req: &ResolveRequest) -> String {
    let player = if req.player == Some(Player::Unity) {
        "Unity"
    } else {
        "AVPro"
    };
    if let Some(h) = req.max_height.filter(|h| *h > 0) {
        return format!("{player} {h}p");
    }
    if let Some(cap) = req.vrchat_format_arg.as_deref().and_then(first_height_cap) {
        return format!("{player} {cap}p");
    }
    format!("{player} max")
}

fn first_height_cap(format_arg: &str) -> Option<&str> {
    let mut rest = format_arg;
    while let Some(i) = rest.find("height<=") {
        let after = &rest[i + "height<=".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            return Some(&after[..digits]);
        }
        rest = after;
    }
    None
}

pub fn cid_suffix(correlation_id: Option<&str>) -> String {
    match correlation_id {
        Some(c) if !c.is_empty() => format!(" cid={}", logutil::sanitize_for_console(c, 64)),
        _ => String::new(),
    }
}

pub fn looks_like_wrapper_event(line: &str) -> bool {
    let end = line.char_indices().nth(256).map(|(i, _)| i).unwrap_or(line.len());
    let head = &line[..end];
    head.contains(action::OG_FALLBACK_NOTIFY) || head.contains(action::WRAPPER_OG_FAILED)
}
