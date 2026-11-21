use serde_json::{Value, json};

pub const CLIENT_NAME_ID: &str = "101";
pub const CLIENT_VERSION: &str = "1.02";
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
pub const SIGN_IN_WALL: &str = "sign_in_wall";
pub const VIDEO_LOCKED: &str = "video_locked";
const LOCKED_HINTS: [&str; 6] = [
    "your age",
    "age-restricted",
    "inappropriate",
    "private",
    "member",
    "purchase",
];
const GUARD_PREFIX: &str = ")]}'";

pub fn video_id(url: &str) -> Option<String> {
    vrcr_shared::source_url::youtube_video_id(url)
}

pub fn visitor_data(body: &str) -> Option<String> {
    let text = body.trim_start();
    let text = text.strip_prefix(GUARD_PREFIX).unwrap_or(text);
    let value: Value = serde_json::from_str(text).ok()?;
    let visitor = value.get(0)?.get(2)?.get(0)?.get(0)?.get(13)?.as_str()?;
    (!visitor.is_empty() && visitor.len() <= 512 && visitor.is_ascii()).then(|| visitor.to_owned())
}

pub fn player_body(video_id: &str, visitor: &str) -> String {
    json!({
        "context": {
            "client": {
                "clientName": "VISIONOS",
                "clientVersion": CLIENT_VERSION,
                "deviceMake": "Apple",
                "deviceModel": "RealityDevice17,1",
                "osName": "visionOS",
                "osVersion": "26.5.23O471",
                "hl": "en",
                "gl": "US",
                "visitorData": visitor,
            }
        },
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    })
    .to_string()
}

pub fn manifest_url(player: &str, video_id: &str) -> Result<String, &'static str> {
    let value: Value = serde_json::from_str(player)
        .ok()
        .ok_or("player_unreadable")?;
    let status = value
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str);
    match status {
        Some("OK") => {}
        Some("LOGIN_REQUIRED") => return Err(login_wall(&value)),
        _ => return Err("not_playable"),
    }
    let details = value.get("videoDetails");
    let answered_id = details
        .and_then(|d| d.get("videoId"))
        .and_then(Value::as_str);
    if answered_id != Some(video_id) {
        return Err("wrong_video");
    }
    let is_live = details
        .and_then(|d| d.get("isLive"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_live {
        return Err("live");
    }
    value
        .pointer("/streamingData/hlsManifestUrl")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .ok_or("no_hls")
}

fn login_wall(value: &Value) -> &'static str {
    let status = value.get("playabilityStatus");
    let messages = status
        .and_then(|s| s.get("messages"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let text = status
        .and_then(|s| s.get("reason"))
        .into_iter()
        .chain(messages)
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let is_locked = !text.contains("bot") && LOCKED_HINTS.iter().any(|hint| text.contains(hint));
    if is_locked {
        VIDEO_LOCKED
    } else {
        SIGN_IN_WALL
    }
}
