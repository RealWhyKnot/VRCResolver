use std::time::SystemTime;

use rmpv::Value as Mp;
use serde_json::Value;
use tokio::time::Instant;

use vrcr_shared::logutil::{iso8601_utc, sanitize_for_console};
use vrcr_shared::wire::{PlaybackFeedbackFrame, ResolveResponse, action, fallback};

use super::connect::Probe;
use crate::ipc::MeshResult;

pub const WIRE_DEADLINE_MARGIN_MS: i64 = 500;
pub const PING_FRAME: &str = r#"{"action":"ping"}"#;
pub const PONG_FRAME: &str = r#"{"action":"pong"}"#;

pub fn wire_deadline_for(deadline: Instant, now: Instant) -> i32 {
    let remaining = if deadline >= now {
        i64::try_from((deadline - now).as_millis()).unwrap_or(i64::MAX)
    } else {
        -i64::try_from((now - deadline).as_millis()).unwrap_or(i64::MAX)
    };
    let clamped = remaining
        .saturating_sub(WIRE_DEADLINE_MARGIN_MS)
        .clamp(0, i64::from(i32::MAX));
    i32::try_from(clamped).unwrap_or(i32::MAX)
}

pub fn fallback_result(id: &str, reason: &str) -> MeshResult {
    let frame = ResolveResponse {
        action: action::FALLBACK_NATIVE.to_owned(),
        id: id.to_owned(),
        reason: Some(reason.to_owned()),
        ..Default::default()
    };
    MeshResult {
        frame: serde_json::to_vec(&frame).unwrap_or_default(),
        action: action::FALLBACK_NATIVE.to_owned(),
        reason: Some(reason.to_owned()),
        retry_after_ms: None,
    }
}

pub fn describe_connect_failure(host: &str, elapsed_ms: u128, probe: &Probe, cause: Option<&str>) -> String {
    let place = format!(
        " after {elapsed_ms} ms during {}{}",
        probe.phase.as_str(),
        probe.timings()
    );
    match cause {
        None => format!("connect to {host} timed out{place}"),
        Some(c) => format!(
            "connect to {host} failed{place}: {}",
            sanitize_for_console(c, 200)
        ),
    }
}

pub fn fallback_native_line(id: &str, reason: Option<&str>) -> String {
    let reason = sanitize_for_console(reason.unwrap_or_default(), 64);
    match reason.as_str() {
        fallback::UNITY_UNSUPPORTED_FORMAT => format!(
            "[mesh] fallback_native id={id} reason=unity_unsupported_format (no Unity-playable stream -- try AVPro)"
        ),
        fallback::WARP_DOWN => format!(
            "[mesh] fallback_native id={id} reason=warp_down (server WARP egress unhealthy -- transient, retry shortly or another node)"
        ),
        "" => format!("[mesh] fallback_native id={id} reason=?"),
        r => format!("[mesh] fallback_native id={id} reason={r}"),
    }
}

pub fn str_field<'a>(root: &'a Value, name: &str) -> Option<&'a str> {
    root.get(name).and_then(Value::as_str)
}

pub fn i32_field(root: &Value, name: &str) -> Option<i32> {
    root.get(name)
        .and_then(Value::as_i64)
        .and_then(|n| i32::try_from(n).ok())
}

pub fn playback_quality_line(root: &Value) -> String {
    let mut line = String::from("[mesh] playback quality");
    if let Some(cid) = str_field(root, "correlation_id") {
        line.push_str(" cid=");
        line.push_str(&sanitize_for_console(cid, 32));
    }
    if let Some(h) = i32_field(root, "delivered_height") {
        line.push_str(&format!(" delivered={h}p"));
    }
    if let Some(h) = i32_field(root, "resolved_height") {
        line.push_str(&format!(" resolved={h}p"));
    }
    line
}

pub fn resolve_log_line(root: &Value) -> String {
    format!(
        "[mesh][resolve_log] id={} level={} {}",
        sanitize_for_console(str_field(root, "id").unwrap_or_default(), 32),
        sanitize_for_console(str_field(root, "level").unwrap_or("info"), 16),
        sanitize_for_console(str_field(root, "message").unwrap_or_default(), 240)
    )
}

#[allow(clippy::too_many_arguments)]
pub fn playback_feedback_payload(
    url: &str,
    kind: &str,
    ms_since_open: i32,
    client_id: &str,
    correlation_id: Option<&str>,
    timestamp: SystemTime,
    delivered_height: Option<i32>,
    detail: Option<&str>,
) -> String {
    let frame = PlaybackFeedbackFrame {
        action: action::PLAYBACK_FEEDBACK.to_owned(),
        url: url.to_owned(),
        kind: kind.to_owned(),
        timestamp: iso8601_utc(timestamp),
        ms_since_open,
        client_id: client_id.to_owned(),
        correlation_id: correlation_id.filter(|c| !c.is_empty()).map(str::to_owned),
        delivered_height: delivered_height.filter(|h| *h > 0),
        detail: detail.filter(|d| !d.is_empty()).map(str::to_owned),
    };
    serde_json::to_string(&frame).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinaryFrame {
    Resolved(ResolveResponse),
    FallbackNative(ResolveResponse),
    ResolveLog { id: String, message: String },
    Ping,
    Pong,
    Other(String),
}

pub fn decode_binary(payload: &[u8]) -> Result<BinaryFrame, String> {
    let mut cursor = payload;
    let value = rmpv::decode::read_value(&mut cursor).map_err(|e| format!("action peek: {e}"))?;
    let Mp::Array(items) = value else {
        return Err("action peek: expected a msgpack array".to_owned());
    };
    let Some(head) = items.first() else {
        return Err("empty msgpack array (count=0)".to_owned());
    };
    let name = match head {
        Mp::Nil => String::new(),
        Mp::String(s) => s
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "action peek: action is not valid utf-8".to_owned())?,
        _ => return Err("action peek: action is not a string".to_owned()),
    };
    match name.as_str() {
        action::RESOLVED => {
            let f = Fields {
                items: &items,
                what: "resolved",
            };
            Ok(BinaryFrame::Resolved(ResolveResponse {
                action: action::RESOLVED.to_owned(),
                id: f.text(1)?.unwrap_or_default(),
                url: f.text(2)?,
                engine: f.text(3)?,
                config: f.text(4)?,
                container: f.text(5)?,
                video_codec: f.text(6)?,
                audio_codec: f.text(7)?,
                protocol: f.text(8)?,
                audio_channels: f.int32(9)?,
                bytes_estimate: f.int64(10)?,
                expires_at: f.text(11)?,
                resolved_height: f.int32(12)?,
                ..Default::default()
            }))
        }
        action::FALLBACK_NATIVE => {
            let f = Fields {
                items: &items,
                what: "fallback_native",
            };
            Ok(BinaryFrame::FallbackNative(ResolveResponse {
                action: action::FALLBACK_NATIVE.to_owned(),
                id: f.text(1)?.unwrap_or_default(),
                reason: f.text(2)?,
                public_message: f.text(3)?,
                retry_after_ms: f.int32(4)?,
                ..Default::default()
            }))
        }
        action::RESOLVE_LOG => {
            let f = Fields {
                items: &items,
                what: "resolve_log",
            };
            Ok(BinaryFrame::ResolveLog {
                id: f.text(1)?.unwrap_or_default(),
                message: f.text(2)?.unwrap_or_default(),
            })
        }
        action::PING => Ok(BinaryFrame::Ping),
        action::PONG => Ok(BinaryFrame::Pong),
        _ => Ok(BinaryFrame::Other(name)),
    }
}

struct Fields<'a> {
    items: &'a [Mp],
    what: &'static str,
}

impl Fields<'_> {
    fn text(&self, i: usize) -> Result<Option<String>, String> {
        match self.items.get(i) {
            None | Some(Mp::Nil) => Ok(None),
            Some(Mp::String(s)) => s
                .as_str()
                .map(|s| Some(s.to_owned()))
                .ok_or_else(|| format!("{} deserialize: field {i} is not valid utf-8", self.what)),
            Some(_) => Err(format!("{} deserialize: field {i} is not a string", self.what)),
        }
    }

    fn int64(&self, i: usize) -> Result<Option<i64>, String> {
        match self.items.get(i) {
            None | Some(Mp::Nil) => Ok(None),
            Some(Mp::Integer(n)) => n
                .as_i64()
                .map(Some)
                .ok_or_else(|| format!("{} deserialize: field {i} is out of range", self.what)),
            Some(_) => Err(format!("{} deserialize: field {i} is not an integer", self.what)),
        }
    }

    fn int32(&self, i: usize) -> Result<Option<i32>, String> {
        match self.int64(i)? {
            None => Ok(None),
            Some(n) => i32::try_from(n)
                .map(Some)
                .map_err(|_| format!("{} deserialize: field {i} is out of range", self.what)),
        }
    }
}
