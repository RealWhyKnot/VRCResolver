use url::Url;

use crate::first_party::allowed_extension;

const YOUTUBE_ID_LENGTH: usize = 11;
const YOUTUBE_ID_PATHS: [&str; 4] = ["shorts", "embed", "live", "v"];
const YOUTUBE_WATCH_HOSTS: [&str; 5] = [
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "www.youtube-nocookie.com",
];
const YOUTUBE_SHORT_HOST: &str = "youtu.be";
const TRACKING_PARAMS: [&str; 5] = ["si", "pp", "feature", "fbclid", "igshid"];
const TRACKING_PREFIX: &str = "utm_";

pub fn is_spotify_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(host.as_str(), "spotify.com" | "spotify.link" | "spoti.fi")
        || host.ends_with(".spotify.com")
}

pub fn normalize(url: &str) -> String {
    let trimmed = url.trim();
    if let Some(id) = youtube_video_id(trimmed) {
        return format!("https://www.youtube.com/watch?v={id}");
    }
    let Ok(mut parsed) = Url::parse(trimmed) else {
        return trimmed.to_owned();
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return trimmed.to_owned();
    }
    parsed.set_fragment(None);
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !is_tracking_param(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    parsed.into()
}

pub fn youtube_video_id(url: &str) -> Option<String> {
    let url = Url::parse(url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let mut segments = url.path_segments()?;
    let first = segments.next().unwrap_or_default();
    let id = if host == YOUTUBE_SHORT_HOST {
        first.to_owned()
    } else if !YOUTUBE_WATCH_HOSTS.contains(&host.as_str()) {
        return None;
    } else if first == "watch" {
        url.query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned())?
    } else if YOUTUBE_ID_PATHS.contains(&first) {
        segments.next()?.to_owned()
    } else {
        return None;
    };
    is_youtube_id(&id).then_some(id)
}

pub fn is_direct_media(url: &str) -> bool {
    !allowed_extension(url.trim()).is_empty()
}

pub fn same_source(a: &str, b: &str) -> bool {
    a == b || normalize(a) == normalize(b)
}

fn is_youtube_id(id: &str) -> bool {
    id.len() == YOUTUBE_ID_LENGTH
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_tracking_param(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    TRACKING_PARAMS.contains(&lower.as_str()) || lower.starts_with(TRACKING_PREFIX)
}
