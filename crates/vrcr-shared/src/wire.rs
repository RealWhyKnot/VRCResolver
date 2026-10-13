use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PIPE_NAME: &str = "vrcresolver.resolve";
pub const WATCHDOG_MUTEX: &str = "Global\\vrcresolver.Watchdog";
pub const WATCHDOG_MUTEX_LOCAL: &str = "Local\\vrcresolver.Watchdog";
pub const CLIENT_PROTOCOL_VERSION: i32 = 3;
pub const SUBPROTOCOL_V3: &str = "vrcresolver-v3";

pub mod action {
    pub const RESOLVE: &str = "resolve";
    pub const RESOLVED: &str = "resolved";
    pub const FALLBACK_NATIVE: &str = "fallback_native";
    pub const RESOLVE_LOG: &str = "resolve_log";
    pub const PING: &str = "ping";
    pub const PONG: &str = "pong";
    pub const WELCOME: &str = "welcome";
    pub const CLIENT_HELLO: &str = "client_hello";
    pub const WELCOME_CACHED: &str = "welcome_cached";
    pub const PLAYBACK_FEEDBACK: &str = "playback_feedback";
    pub const PLAYBACK_QUALITY_UPDATE: &str = "playback_quality_update";
    pub const PROTOCOL_ERROR: &str = "protocol_error";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const OG_FALLBACK_NOTIFY: &str = "og_fallback_notify";
    pub const WRAPPER_OG_FAILED: &str = "wrapper_og_failed";
}

pub mod feedback {
    pub const LOAD_FAILURE: &str = "load_failure";
    pub const SILENT_STALL: &str = "silent_stall";
    pub const PLAYING: &str = "playing";
    pub const RESOLVED_REJECTED: &str = "resolved_rejected";
    pub const OG_FAILED: &str = "og_failed";
    pub const CACHE_PLAY: &str = "cache_play";
}

pub mod og_reason {
    pub const PIPE_CONNECT_FAILED: &str = "pipe_connect_failed";
    pub const PIPE_RESOLVE_FAILED: &str = "pipe_resolve_failed";
    pub const SERVER_FALLBACK_NATIVE: &str = "server_fallback_native";
    pub const NO_URL_DIAGNOSTIC: &str = "no_url_diagnostic";
    pub const PRIOR_LOAD_FAILURE: &str = "prior_load_failure";
    pub const AVPRO_INCOMPATIBLE: &str = "avpro_incompatible";
    pub const RESOLVED_URL_REJECTED: &str = "resolved_url_rejected";
    pub const RESOLVER_UNHEALTHY: &str = "resolver_unhealthy";
}

pub mod fallback {
    pub const ALL_CONFIGS_FAILED: &str = "all_configs_failed";
    pub const EXTRACTOR_UNSUPPORTED: &str = "extractor_unsupported";
    pub const INTERNAL_ERROR: &str = "internal_error";
    pub const DISCOVERY_IN_PROGRESS: &str = "discovery_in_progress";
    pub const SERVER_UNREACHABLE: &str = "server_unreachable";
    pub const CLIENT_DEADLINE_EXCEEDED: &str = "client_deadline_exceeded";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const PROTOCOL_ERROR: &str = "protocol_error";
    pub const UNITY_UNSUPPORTED_FORMAT: &str = "unity_unsupported_format";
    pub const WARP_DOWN: &str = "warp_down";
    pub const VALIDATION_FAILED_PREFIX: &str = "validation_failed_";
}

pub mod field {
    pub const RETRY_AFTER_SECONDS: &str = "retryAfterSeconds";
    pub const MESH_ACTION: &str = "meshAction";
}

pub mod format {
    pub const JSON: &str = "json";
    pub const MSGPACK: &str = "msgpack";
    pub const PREFERENCE: [&str; 2] = [MSGPACK, JSON];
    pub const JSON_ONLY: [&str; 1] = [JSON];
}

pub mod feature {
    pub const WELCOME_HOSTS: &str = "welcome_hosts";
    pub const PLAYBACK_FEEDBACK: &str = "playback_feedback";
    pub const PLAYBACK_FEEDBACK_V2: &str = "playback_feedback_v2";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Player {
    AvPro,
    Unity,
    #[serde(other)]
    Unknown,
}

impl Player {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AvPro => "avpro",
            Self::Unity => "unity",
            Self::Unknown => "unknown",
        }
    }

    pub fn accept_protocols(self) -> &'static [&'static str] {
        match self {
            Self::Unity => &UNITY_ACCEPT_PROTOCOLS,
            _ => &AVPRO_ACCEPT_PROTOCOLS,
        }
    }

    pub fn accept_codecs(self) -> &'static [&'static str] {
        match self {
            Self::Unity => &UNITY_ACCEPT_CODECS,
            _ => &AVPRO_ACCEPT_CODECS,
        }
    }

    pub fn max_audio_channels(self) -> i32 {
        match self {
            Self::Unity => UNITY_MAX_AUDIO_CHANNELS,
            _ => AVPRO_MAX_AUDIO_CHANNELS,
        }
    }
}

pub const AVPRO_ACCEPT_PROTOCOLS: [&str; 3] = ["http", "hls", "dash"];
pub const UNITY_ACCEPT_PROTOCOLS: [&str; 1] = ["http"];
pub const AVPRO_ACCEPT_CODECS: [&str; 9] =
    ["h264", "h265", "vp9", "av1", "aac", "opus", "mp3", "ac3", "eac3"];
pub const UNITY_ACCEPT_CODECS: [&str; 2] = ["h264", "aac"];
pub const AVPRO_MAX_AUDIO_CHANNELS: i32 = 8;
pub const UNITY_MAX_AUDIO_CHANNELS: i32 = 2;
pub const HIGH_QUALITY_MAX_HEIGHT: i32 = 2160;
pub const DEFAULT_MAX_HEIGHT: i32 = 1080;
pub const EXTENSION_BACKED_VIDEO_CODECS: [&str; 3] = ["h265", "vp9", "av1"];

pub fn build_accept_codecs<S: AsRef<str>>(verified_video_codecs: Option<&[S]>) -> Vec<&'static str> {
    AVPRO_ACCEPT_CODECS
        .iter()
        .copied()
        .filter(|codec| {
            !EXTENSION_BACKED_VIDEO_CODECS.contains(codec)
                || verified_video_codecs.is_some_and(|v| v.iter().any(|c| c.as_ref() == *codec))
        })
        .collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResolveRequest {
    pub action: String,
    pub id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<Player>,
    #[serde(rename = "maxHeight", skip_serializing_if = "Option::is_none")]
    pub max_height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_protocols: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_codecs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_audio_channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vrchat_format_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper_deadline_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer_highest: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_native_hint: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ResolveRequest {
    fn default() -> Self {
        Self {
            action: action::RESOLVE.to_owned(),
            id: String::new(),
            url: String::new(),
            player: None,
            max_height: None,
            protocol_version: None,
            correlation_id: None,
            accept_protocols: None,
            accept_codecs: None,
            max_audio_channels: None,
            vrchat_format_arg: None,
            wrapper_deadline_ms: None,
            prefer_highest: None,
            skip_native_hint: None,
            extra: Map::new(),
        }
    }
}

impl ResolveRequest {
    pub fn for_player(id: String, url: String, player: Player) -> Self {
        Self {
            id,
            url,
            player: Some(player),
            protocol_version: Some(CLIENT_PROTOCOL_VERSION),
            accept_protocols: Some(strings(player.accept_protocols())),
            accept_codecs: Some(strings(player.accept_codecs())),
            max_audio_channels: Some(player.max_audio_channels()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResolveResponse {
    pub action: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_estimate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<i32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WelcomeFrame {
    pub action: String,
    pub protocol_version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warp_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engines: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yt_dlp_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub welcome_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negotiated_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_party_hosts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_proxy_paths: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientHelloFrame {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub welcome_hash: Option<String>,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_formats: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WelcomeCachedFrame {
    pub action: String,
    pub protocol_version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warp_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negotiated_format: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackFeedbackFrame {
    pub action: String,
    pub url: String,
    pub kind: String,
    pub timestamp: String,
    pub ms_since_open: i32,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WrapperEventNotify {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub elapsed_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rid: Option<String>,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_message: Option<String>,
}

pub fn to_line<T: Serialize>(frame: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = serde_json::to_vec(frame)?;
    out.push(b'\n');
    Ok(out)
}
