use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmpv::Value as Mp;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

use vrcr_shared::console::{Style, Ux};
use vrcr_shared::first_party::ALLOWED_PATH_EXTENSIONS;
use vrcr_shared::log::Log;
use vrcr_shared::wire::{
    self, CLIENT_PROTOCOL_VERSION, Player, ResolveRequest, ResolveResponse, SUBPROTOCOL_V3,
    WelcomeFrame, action, fallback, feature, feedback, og_reason,
};
use vrcr_watchdog::ipc::server::{
    OG_FAILED_DETAIL_UNKNOWN, OG_FAILED_DETAILS, effective_budget_ms, og_failed_detail_for,
};
use vrcr_watchdog::mesh::connect::{
    ConnectError, Phase, Probe, check_upgrade_response, upgrade_request,
};
use vrcr_watchdog::mesh::frames::{
    BinaryFrame, decode_binary, describe_connect_failure, fallback_native_line,
    playback_feedback_payload, playback_quality_line, resolve_log_line, wire_deadline_for,
};
use vrcr_watchdog::mesh::identity::{self, load_or_create, load_or_create_with, parse_simple_guid};
use vrcr_watchdog::mesh::reporting::{
    MAX_PER_SESSION, MAX_SUMMARY_CHARS, MIN_INTERVAL, Machine, Post, ReportPayload, Reporter,
    extract_domain, failure_kind, hash_short, path_and_query, replace_ignore_ascii_case,
};
use vrcr_watchdog::mesh::welcome_cache::{self, WelcomeCache};
use vrcr_watchdog::mesh::{
    RECONNECT_CAPS_SEC, draw_backoff_ms, mesh_url_for_host, should_send_client_hello,
};

use crate::common::mesh::Capture;

const CONTRACT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/mesh-contract.json"
));

fn contract() -> Value {
    serde_json::from_str(CONTRACT).unwrap_or_else(|e| panic!("{e}"))
}

fn strings(root: &Value, name: &str) -> Vec<String> {
    let mut out: Vec<String> = root[name]
        .as_array()
        .unwrap_or_else(|| panic!("{name} is not an array"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("{name} holds a non-string"))
                .to_owned()
        })
        .collect();
    out.sort();
    out
}

fn sorted(items: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = items.iter().map(|s| (*s).to_owned()).collect();
    out.sort();
    out
}

fn number(root: &Value, section: &str, name: &str) -> i32 {
    root[section][name]
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or_else(|| panic!("{section}.{name} is not an i32"))
}

fn json_sample(root: &Value, name: &str) -> Value {
    let text = root["json_samples"][name]
        .as_str()
        .unwrap_or_else(|| panic!("no json sample for {name}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}"))
}

fn msgpack_sample(root: &Value, name: &str) -> Vec<u8> {
    let text = root["msgpack_samples_hex"][name]
        .as_str()
        .unwrap_or_else(|| panic!("no msgpack sample for {name}"));
    hex::decode(text).unwrap_or_else(|e| panic!("{e}"))
}

fn text(root: &Value, name: &str) -> Option<String> {
    root.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn int(root: &Value, name: &str) -> Option<i64> {
    root.get(name).and_then(Value::as_i64)
}

fn encode(items: Vec<Mp>) -> Vec<u8> {
    let mut out = Vec::new();
    rmpv::encode::write_value(&mut out, &Mp::Array(items)).unwrap_or_else(|e| panic!("{e}"));
    out
}

fn s(v: &str) -> Mp {
    Mp::from(v)
}

fn resolved(payload: &[u8]) -> ResolveResponse {
    match decode_binary(payload) {
        Ok(BinaryFrame::Resolved(r)) => r,
        other => panic!("expected resolved, got {other:?}"),
    }
}

fn fallback_frame(payload: &[u8]) -> ResolveResponse {
    match decode_binary(payload) {
        Ok(BinaryFrame::FallbackNative(r)) => r,
        other => panic!("expected fallback_native, got {other:?}"),
    }
}

fn is_simple_guid(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn probe(phase: Phase, dns_ms: Option<u64>, tcp_ms: Option<u64>) -> Probe {
    Probe {
        phase,
        dns_ms,
        tcp_ms,
    }
}

fn ok_head(key: &str, extra: &str) -> String {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n{extra}\r\n",
        derive_accept_key(key.as_bytes())
    )
}

const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

struct IdFile {
    _dir: TempDir,
    dir: PathBuf,
    path: PathBuf,
}

fn id_file() -> IdFile {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path().to_path_buf();
    IdFile {
        path: root.join(identity::FILE_NAME),
        dir: root,
        _dir: dir,
    }
}

fn read_trimmed(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{e}"))
        .trim()
        .to_owned()
}

#[cfg(windows)]
fn lock_exclusively(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap_or_else(|e| panic!("{e}"))
}

fn list(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{e}"))
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

struct CacheDir {
    _dir: TempDir,
    dir: PathBuf,
    path: PathBuf,
}

fn cache_dir() -> CacheDir {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path().to_path_buf();
    CacheDir {
        path: root.join(welcome_cache::FILE_NAME),
        dir: root,
        _dir: dir,
    }
}

fn cache(c: &CacheDir) -> WelcomeCache {
    WelcomeCache::new(c.path.clone(), Log::disabled())
}

fn welcome(node: Option<&str>) -> WelcomeFrame {
    WelcomeFrame {
        protocol_version: 3,
        node: node.map(str::to_owned),
        ..WelcomeFrame::default()
    }
}

fn owned(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|s| (*s).to_owned()).collect())
}

fn machine() -> Machine {
    Machine {
        user_name: Some("alice".to_owned()),
        user_profile: Some("C:\\Users\\alice".to_owned()),
        machine_name: Some("DESKTOP-1".to_owned()),
    }
}

type Sent = Arc<Mutex<Vec<ReportPayload>>>;

fn recording_post(status: u16) -> (Post, Sent) {
    let sent: Sent = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&sent);
    let post: Post = Arc::new(move |p: &ReportPayload| {
        keep.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(p.clone());
        Ok(status)
    });
    (post, sent)
}

fn reporter(enabled: bool, post: Post) -> (Reporter, Capture) {
    let out = Capture::default();
    let ux = out.ux();
    (Reporter::new(enabled, "1.2.3", ux, machine(), post), out)
}

fn yt_request() -> ResolveRequest {
    ResolveRequest {
        id: "r1".to_owned(),
        url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=5".to_owned(),
        player: Some(Player::Unity),
        ..ResolveRequest::default()
    }
}

fn wait_for(sent: &Sent, count: usize) {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while sent.lock().unwrap_or_else(|e| e.into_inner()).len() < count {
        assert!(std::time::Instant::now() < until, "report never posted");
        std::thread::sleep(Duration::from_millis(10));
    }
}

mod cache_cases;

mod contracts_cases;

mod frames_cases;

mod identity_cases;

mod reporting_cases;

mod requests_cases;

mod upgrade_cases;
