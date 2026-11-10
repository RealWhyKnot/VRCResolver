use serde_json::{Value, json};
use url::Url;

pub const CLIENT_NAME_ID: &str = "101";
pub const CLIENT_VERSION: &str = "1.02";
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
pub const SIGN_IN_WALL: &str = "sign_in_wall";
const GUARD_PREFIX: &str = ")]}'";
const ID_LENGTH: usize = 11;
const ID_PATHS: [&str; 4] = ["shorts", "embed", "live", "v"];
const WATCH_HOSTS: [&str; 5] = [
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "www.youtube-nocookie.com",
];
const SHORT_HOST: &str = "youtu.be";

pub fn video_id(url: &str) -> Option<String> {
    let url = Url::parse(url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let mut segments = url.path_segments()?;
    let first = segments.next().unwrap_or_default();
    let id = if host == SHORT_HOST {
        first.to_owned()
    } else if !WATCH_HOSTS.contains(&host.as_str()) {
        return None;
    } else if first == "watch" {
        url.query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned())?
    } else if ID_PATHS.contains(&first) {
        segments.next()?.to_owned()
    } else {
        return None;
    };
    is_video_id(&id).then_some(id)
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
        Some("LOGIN_REQUIRED") => return Err(SIGN_IN_WALL),
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

fn is_video_id(id: &str) -> bool {
    id.len() == ID_LENGTH
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
