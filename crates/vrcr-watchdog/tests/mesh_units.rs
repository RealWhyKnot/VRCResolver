mod common;

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
    self, CLIENT_PROTOCOL_VERSION, Player, ResolveRequest, ResolveResponse, SUBPROTOCOL_V3, WelcomeFrame,
    action, fallback, feature, feedback, og_reason,
};
use vrcr_watchdog::ipc::server::{
    OG_FAILED_DETAIL_UNKNOWN, OG_FAILED_DETAILS, effective_budget_ms, og_failed_detail_for,
};
use vrcr_watchdog::mesh::connect::{ConnectError, Phase, Probe, check_upgrade_response, upgrade_request};
use vrcr_watchdog::mesh::frames::{
    BinaryFrame, decode_binary, describe_connect_failure, fallback_native_line, playback_feedback_payload,
    playback_quality_line, resolve_log_line, wire_deadline_for,
};
use vrcr_watchdog::mesh::identity::{self, load_or_create, parse_simple_guid};
use vrcr_watchdog::mesh::reporting::{
    MAX_PER_SESSION, MAX_SUMMARY_CHARS, MIN_INTERVAL, Machine, Post, ReportPayload, Reporter, extract_domain,
    failure_kind, hash_short, path_and_query, replace_ignore_ascii_case,
};
use vrcr_watchdog::mesh::welcome_cache::{self, WelcomeCache};
use vrcr_watchdog::mesh::{RECONNECT_CAPS_SEC, draw_backoff_ms, mesh_url_for_host, should_send_client_hello};

use common::mesh::Capture;

const CONTRACT: &str = include_str!("fixtures/mesh-contract.json");

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

#[test]
fn upgrade_request_offers_v3_and_no_extensions() {
    let req = upgrade_request("/mesh", "vrcresolver.com", "VRCResolver/1.2.3", KEY);
    assert!(req.starts_with("GET /mesh HTTP/1.1\r\n"));
    assert!(req.contains("\r\nHost: vrcresolver.com\r\n"));
    assert!(req.contains("\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"));
    assert!(req.contains(&format!("\r\nSec-WebSocket-Protocol: {SUBPROTOCOL_V3}\r\n")));
    assert!(req.contains("\r\nUser-Agent: VRCResolver/1.2.3\r\n"));
    assert!(req.contains("\r\nSec-WebSocket-Version: 13\r\n"));
    assert!(!req.to_ascii_lowercase().contains("sec-websocket-extensions"));
    assert!(req.ends_with("\r\n\r\n"));
}

#[test]
fn upgrade_response_with_v3_is_accepted() {
    let head = ok_head(KEY, &format!("Sec-WebSocket-Protocol: {SUBPROTOCOL_V3}\r\n"));
    assert_eq!(
        check_upgrade_response(&head, KEY),
        Ok(Some(SUBPROTOCOL_V3.to_owned()))
    );
}

#[test]
fn upgrade_response_without_a_subprotocol_is_legacy() {
    assert_eq!(check_upgrade_response(&ok_head(KEY, ""), KEY), Ok(None));
}

#[test]
fn upgrade_response_headers_are_case_insensitive() {
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: WebSocket\r\nconnection: keep-alive, Upgrade\r\nsec-websocket-accept: {}\r\n\r\n",
        derive_accept_key(KEY.as_bytes())
    );
    assert_eq!(check_upgrade_response(&head, KEY), Ok(None));
}

#[test]
fn upgrade_response_rejections() {
    let cases = [
        (
            "HTTP/1.1 503 Service Unavailable\r\n\r\n".to_owned(),
            "server returned HTTP 503 instead of 101",
        ),
        (
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                derive_accept_key(KEY.as_bytes())
            ),
            "upgrade response is missing Upgrade: websocket",
        ),
        (
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                derive_accept_key(KEY.as_bytes())
            ),
            "upgrade response is missing Connection: Upgrade",
        ),
        (
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: bogus\r\n\r\n"
                .to_owned(),
            "upgrade response carries the wrong Sec-WebSocket-Accept",
        ),
        (
            ok_head(KEY, "Sec-WebSocket-Extensions: permessage-deflate\r\n"),
            "server enabled a websocket extension that was not offered",
        ),
        (
            ok_head(KEY, "Sec-WebSocket-Protocol: vrcresolver-v9\r\n"),
            "server picked a subprotocol that was not offered: vrcresolver-v9",
        ),
    ];
    for (head, want) in cases {
        assert_eq!(check_upgrade_response(&head, KEY), Err(want.to_owned()), "{head}");
    }
}

#[test]
fn connect_error_names_the_kind() {
    let io = || std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
    assert_eq!(ConnectError::Url("bad".to_owned()).describe(), "UrlError: bad");
    assert_eq!(ConnectError::Dns(io()).describe(), "DnsError: refused");
    assert_eq!(ConnectError::Tcp(io()).describe(), "TcpError: refused");
    assert_eq!(ConnectError::Tls(io()).describe(), "TlsError: refused");
    assert_eq!(
        ConnectError::Handshake("x".to_owned()).describe(),
        "HandshakeError: x"
    );
}

#[test]
fn describe_connect_failure_names_the_phase() {
    let cases = [
        (
            probe(Phase::Tcp, Some(12), None),
            None,
            "connect to vrcresolver.com timed out after 8000 ms during tcp (dns 12 ms)",
        ),
        (
            probe(Phase::Upgrade, Some(3), Some(40)),
            None,
            "connect to vrcresolver.com timed out after 8000 ms during upgrade (dns 3 ms, tcp 40 ms)",
        ),
        (
            probe(Phase::Upgrade, Some(3), Some(40)),
            Some("HandshakeError: x"),
            "connect to vrcresolver.com failed after 8000 ms during upgrade (dns 3 ms, tcp 40 ms): HandshakeError: x",
        ),
    ];
    for (p, cause, want) in cases {
        assert_eq!(describe_connect_failure("vrcresolver.com", 8000, &p, cause), want);
    }
}

#[test]
fn describe_connect_failure_omits_timings_that_were_never_recorded() {
    assert_eq!(
        describe_connect_failure("vrcresolver.com", 8000, &probe(Phase::Dns, None, None), None),
        "connect to vrcresolver.com timed out after 8000 ms during dns"
    );
}

#[test]
fn describe_connect_failure_reports_the_root_cause() {
    let cause = ConnectError::Handshake("server returned HTTP 503 instead of 101".to_owned()).describe();
    assert_eq!(
        describe_connect_failure(
            "vrcresolver.com",
            15,
            &probe(Phase::Upgrade, Some(1), Some(2)),
            Some(&cause)
        ),
        "connect to vrcresolver.com failed after 15 ms during upgrade (dns 1 ms, tcp 2 ms): HandshakeError: server returned HTTP 503 instead of 101"
    );
}

#[test]
fn describe_connect_failure_strips_control_characters_from_the_cause() {
    let line = describe_connect_failure("h", 1, &probe(Phase::Dns, None, None), Some("a\r\nb"));
    assert!(!line.contains('\r') && !line.contains('\n'), "{line}");
}

#[test]
fn wire_deadline_keeps_a_half_second_margin() {
    let now = Instant::now();
    let cases: [(i64, i32); 5] = [(10_000, 9_500), (501, 1), (500, 0), (300, 0), (-250, 0)];
    for (offset, want) in cases {
        let deadline = if offset >= 0 {
            now + Duration::from_millis(offset.unsigned_abs())
        } else {
            now.checked_sub(Duration::from_millis(offset.unsigned_abs()))
                .unwrap_or(now)
        };
        assert_eq!(wire_deadline_for(deadline, now), want, "offset {offset}");
    }
}

#[test]
fn backoff_draws_stay_inside_their_cap() {
    assert_eq!(RECONNECT_CAPS_SEC, [1, 2, 4, 8, 16, 30]);
    for attempt in 0..12u32 {
        let index = usize::try_from(attempt.saturating_sub(1))
            .unwrap_or(usize::MAX)
            .min(RECONNECT_CAPS_SEC.len() - 1);
        let cap = RECONNECT_CAPS_SEC[index] * 1000;
        let mut max = 0;
        for _ in 0..400 {
            let ms = draw_backoff_ms(attempt);
            assert!(ms <= cap, "attempt {attempt} drew {ms} over {cap}");
            max = max.max(ms);
        }
        assert!(
            max > cap / 2,
            "attempt {attempt} never drew above half the cap ({max} of {cap})"
        );
    }
}

#[test]
fn client_hello_only_on_the_v3_subprotocol() {
    assert!(should_send_client_hello(Some(SUBPROTOCOL_V3)));
    assert!(!should_send_client_hello(Some("vrcresolver-v2")));
    assert!(!should_send_client_hello(Some("")));
    assert!(!should_send_client_hello(None));
}

#[test]
fn mesh_url_trims_the_host() {
    assert_eq!(
        mesh_url_for_host(" vrcresolver.com "),
        "wss://vrcresolver.com/mesh"
    );
}

#[test]
fn fallback_native_lines_explain_the_known_reasons() {
    assert!(fallback_native_line("a", Some(fallback::UNITY_UNSUPPORTED_FORMAT)).contains("try AVPro"));
    assert!(fallback_native_line("a", Some(fallback::WARP_DOWN)).contains("WARP egress unhealthy"));
    assert_eq!(
        fallback_native_line("a", None),
        "[mesh] fallback_native id=a reason=?"
    );
    assert_eq!(
        fallback_native_line("a", Some("all_configs_failed")),
        "[mesh] fallback_native id=a reason=all_configs_failed"
    );
}

#[test]
fn playback_quality_lines() {
    let cases = [
        (
            json!({"correlation_id":"c1","delivered_height":1080,"resolved_height":720}),
            "[mesh] playback quality cid=c1 delivered=1080p resolved=720p",
        ),
        (
            json!({"correlation_id":"c1","delivered_height":1080}),
            "[mesh] playback quality cid=c1 delivered=1080p",
        ),
        (json!({"correlation_id":"c1"}), "[mesh] playback quality cid=c1"),
        (
            json!({"action":"playback_quality_update"}),
            "[mesh] playback quality",
        ),
        (
            json!({"correlation_id":"c\r\n1","delivered_height":1080}),
            "[mesh] playback quality cid=c??1 delivered=1080p",
        ),
    ];
    for (frame, want) in cases {
        assert_eq!(playback_quality_line(&frame), want);
    }
}

#[test]
fn resolve_log_line_defaults_the_level() {
    assert_eq!(
        resolve_log_line(&json!({"id":"r1","message":"trying tv"})),
        "[mesh][resolve_log] id=r1 level=info trying tv"
    );
}

#[test]
fn playback_feedback_payload_drops_empty_optionals() {
    let at = std::time::UNIX_EPOCH + Duration::from_secs(1_790_704_800);
    let text = playback_feedback_payload(
        "https://x",
        feedback::PLAYING,
        1200,
        "cid0",
        Some(""),
        at,
        Some(0),
        Some(""),
    );
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v["action"], action::PLAYBACK_FEEDBACK);
    assert_eq!(v["kind"], feedback::PLAYING);
    assert_eq!(v["ms_since_open"], 1200);
    assert_eq!(v["client_id"], "cid0");
    assert!(v.get("correlation_id").is_none_or(Value::is_null));
    assert!(v.get("delivered_height").is_none_or(Value::is_null));
    assert!(v.get("detail").is_none_or(Value::is_null));
    let text = playback_feedback_payload(
        "https://x",
        feedback::OG_FAILED,
        5,
        "c",
        Some("k1"),
        at,
        Some(720),
        Some("d"),
    );
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v["correlation_id"], "k1");
    assert_eq!(v["delivered_height"], 720);
    assert_eq!(v["detail"], "d");
}

#[test]
fn msgpack_resolved_decodes_every_field() {
    let bytes = encode(vec![
        s("resolved"),
        s("x9k2pq8r4mzj7vn3"),
        s("https://rr1.googlevideo.com/foo"),
        s("yt-dlp"),
        s("youtube-tv-combo"),
        s("mp4"),
        s("h264"),
        s("aac"),
        s("http"),
        Mp::from(2),
        Mp::from(12_345_678i64),
        s("2026-05-04T21:00:00Z"),
        Mp::from(1080),
    ]);
    let r = resolved(&bytes);
    assert_eq!(r.action, "resolved");
    assert_eq!(r.id, "x9k2pq8r4mzj7vn3");
    assert_eq!(r.url.as_deref(), Some("https://rr1.googlevideo.com/foo"));
    assert_eq!(r.engine.as_deref(), Some("yt-dlp"));
    assert_eq!(r.config.as_deref(), Some("youtube-tv-combo"));
    assert_eq!(r.container.as_deref(), Some("mp4"));
    assert_eq!(r.video_codec.as_deref(), Some("h264"));
    assert_eq!(r.audio_codec.as_deref(), Some("aac"));
    assert_eq!(r.protocol.as_deref(), Some("http"));
    assert_eq!(r.audio_channels, Some(2));
    assert_eq!(r.bytes_estimate, Some(12_345_678));
    assert_eq!(r.expires_at.as_deref(), Some("2026-05-04T21:00:00Z"));
    assert_eq!(r.resolved_height, Some(1080));
}

#[test]
fn msgpack_resolved_with_trailing_nils_decodes() {
    let bytes = [
        0x9D, 0xA1, b'R', 0xA1, b'I', 0xA1, b'U', 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0,
    ];
    match decode_binary(&bytes) {
        Ok(BinaryFrame::Other(name)) => assert_eq!(name, "R"),
        other => panic!("{other:?}"),
    }
    let mut named = vec![s("resolved"), s("shortid"), s("https://x")];
    named.extend(std::iter::repeat_n(Mp::Nil, 10));
    let r = resolved(&encode(named));
    assert_eq!(r.id, "shortid");
    assert_eq!(r.engine, None);
    assert_eq!(r.audio_channels, None);
    assert_eq!(r.bytes_estimate, None);
}

#[test]
fn msgpack_resolved_tolerates_extra_trailing_fields() {
    let mut items = vec![s("resolved"), s("id1"), s("https://example.com")];
    items.extend(std::iter::repeat_n(Mp::Nil, 10));
    items.push(s(""));
    items.push(Mp::Nil);
    let r = resolved(&encode(items));
    assert_eq!(r.id, "id1");
    assert_eq!(r.url.as_deref(), Some("https://example.com"));
}

#[test]
fn msgpack_resolved_tolerates_a_short_array_from_an_older_server() {
    let mut items = vec![s("resolved"), s("id1"), s("https://example.com")];
    items.extend(std::iter::repeat_n(Mp::Nil, 9));
    let r = resolved(&encode(items));
    assert_eq!(r.id, "id1");
    assert_eq!(r.resolved_height, None);
}

#[test]
fn msgpack_fallback_tolerates_short_and_four_element_frames() {
    let r = fallback_frame(&encode(vec![s("fallback_native"), s("id1"), s("warp_down")]));
    assert_eq!(r.reason.as_deref(), Some("warp_down"));
    assert_eq!(r.public_message, None);
    assert_eq!(r.retry_after_ms, None);
    let r = fallback_frame(&[
        0x94, 0xAF, b'f', b'a', b'l', b'l', b'b', b'a', b'c', b'k', b'_', b'n', b'a', b't', b'i', b'v', b'e',
        0xA1, b'I', 0xA1, b'R', 0xC0,
    ]);
    assert_eq!(r.id, "I");
    assert_eq!(r.retry_after_ms, None);
}

#[test]
fn msgpack_fallback_carries_retry_after_and_public_message() {
    let r = fallback_frame(&encode(vec![
        s("fallback_native"),
        s("id1"),
        s("discovery_in_progress"),
        Mp::Nil,
        Mp::from(8500),
    ]));
    assert_eq!(r.retry_after_ms, Some(8500));
    let r = fallback_frame(&encode(vec![
        s("fallback_native"),
        s("id1"),
        s("drm_detected"),
        s("Content is DRM-protected and can't be played."),
    ]));
    assert_eq!(
        r.public_message.as_deref(),
        Some("Content is DRM-protected and can't be played.")
    );
}

#[test]
fn msgpack_control_and_log_frames() {
    assert_eq!(decode_binary(&encode(vec![s("ping")])), Ok(BinaryFrame::Ping));
    assert_eq!(decode_binary(&encode(vec![s("pong")])), Ok(BinaryFrame::Pong));
    assert_eq!(
        decode_binary(&encode(vec![
            s("resolve_log"),
            s("id1"),
            s("trying youtube-tv-combo")
        ])),
        Ok(BinaryFrame::ResolveLog {
            id: "id1".to_owned(),
            message: "trying youtube-tv-combo".to_owned()
        })
    );
}

#[test]
fn msgpack_garbage_is_an_error() {
    assert!(decode_binary(&[]).is_err());
    assert!(decode_binary(&[0x90]).is_err());
    assert!(decode_binary(&[0xA1, b'x']).is_err());
    assert!(decode_binary(&encode(vec![Mp::from(5)])).is_err());
    assert!(decode_binary(&encode(vec![s("resolved"), Mp::from(1)])).is_err());
    assert!(
        decode_binary(&encode(vec![
            s("resolved"),
            s("i"),
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::from(i64::MAX)
        ]))
        .is_err()
    );
}

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

#[test]
fn identity_first_call_generates_and_persists() {
    let f = id_file();
    assert!(!f.path.exists());
    let id = load_or_create(&f.path);
    assert!(is_simple_guid(&id), "{id}");
    assert_eq!(read_trimmed(&f.path), id);
}

#[test]
fn identity_later_calls_return_the_same_id() {
    let f = id_file();
    let first = load_or_create(&f.path);
    assert_eq!(load_or_create(&f.path), first);
    assert_eq!(load_or_create(&f.path), first);
}

#[test]
fn identity_corrupt_file_is_regenerated() {
    let f = id_file();
    fs::write(&f.path, "not-a-guid").unwrap_or_else(|e| panic!("{e}"));
    let id = load_or_create(&f.path);
    assert!(is_simple_guid(&id));
    assert_eq!(read_trimmed(&f.path), id);
    assert_eq!(load_or_create(&f.path), id);
}

#[test]
fn identity_surrounding_whitespace_is_accepted() {
    let f = id_file();
    let g = uuid::Uuid::new_v4().simple().to_string();
    fs::write(&f.path, format!("  {g}\r\n")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(load_or_create(&f.path), g);
}

#[test]
fn identity_dashed_guid_is_rejected() {
    let f = id_file();
    let g = uuid::Uuid::new_v4();
    fs::write(&f.path, g.hyphenated().to_string()).unwrap_or_else(|e| panic!("{e}"));
    let id = load_or_create(&f.path);
    assert_ne!(id, g.simple().to_string());
    assert!(is_simple_guid(&id));
}

#[test]
fn identity_write_leaves_no_temp_file() {
    let f = id_file();
    load_or_create(&f.path);
    assert!(f.path.exists());
    assert!(!identity::sibling(&f.path, ".new").exists());
}

#[test]
fn identity_missing_parent_dirs_are_created() {
    let f = id_file();
    let deep = f.dir.join("subdir").join("nested").join(identity::FILE_NAME);
    let id = load_or_create(&deep);
    assert!(is_simple_guid(&id));
    assert!(deep.exists());
}

#[test]
fn identity_oversized_file_is_replaced() {
    let f = id_file();
    let cap = usize::try_from(identity::MAX_FILE_BYTES).unwrap_or(256);
    fs::write(&f.path, vec![0u8; cap + 1]).unwrap_or_else(|e| panic!("{e}"));
    let id = load_or_create(&f.path);
    assert!(is_simple_guid(&id));
    assert_eq!(read_trimmed(&f.path), id);
    assert_eq!(fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0), 32);
}

#[test]
fn identity_parse_lowercases_and_checks_length() {
    assert_eq!(
        parse_simple_guid("ABCDEF0123456789ABCDEF0123456789").as_deref(),
        Some("abcdef0123456789abcdef0123456789")
    );
    assert_eq!(parse_simple_guid("abc"), None);
    assert_eq!(parse_simple_guid("zbcdef0123456789abcdef0123456789"), None);
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

#[cfg(windows)]
#[test]
fn identity_survives_a_brief_lock() {
    let f = id_file();
    let stored = load_or_create(&f.path);
    let lock = lock_exclusively(&f.path);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(lock);
    });
    let id = load_or_create(&f.path);
    release
        .join()
        .unwrap_or_else(|_| panic!("release thread panicked"));
    assert_eq!(id, stored);
    assert_eq!(read_trimmed(&f.path), stored);
}

#[cfg(windows)]
#[test]
fn identity_does_not_overwrite_a_locked_file() {
    let f = id_file();
    let stored = load_or_create(&f.path);
    let before: Vec<_> = list(&f.dir);
    let id = {
        let _lock = lock_exclusively(&f.path);
        load_or_create(&f.path)
    };
    assert!(is_simple_guid(&id));
    assert_ne!(id, stored);
    assert_eq!(read_trimmed(&f.path), stored);
    assert_eq!(list(&f.dir), before);
}

#[cfg(windows)]
#[test]
fn identity_failed_save_cleans_its_temp_file() {
    let f = id_file();
    fs::write(&f.path, "corrupt").unwrap_or_else(|e| panic!("{e}"));
    let id = {
        let _lock = lock_exclusively(&f.path);
        load_or_create(&f.path)
    };
    assert!(is_simple_guid(&id));
    assert!(!identity::sibling(&f.path, ".new").exists());
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

#[test]
fn welcome_cache_missing_file_is_a_miss() {
    let c = cache_dir();
    assert_eq!(cache(&c).get("us1.vrcresolver.com"), None);
}

#[test]
fn welcome_cache_round_trips_per_node() {
    let c = cache_dir();
    let first = WelcomeFrame {
        engines: owned(&["yt-dlp"]),
        features: owned(&["v3_compression"]),
        warp_active: Some(true),
        server_version: Some("v1".to_owned()),
        yt_dlp_version: Some("yd1".to_owned()),
        ..welcome(Some("node1"))
    };
    let second = WelcomeFrame {
        engines: owned(&["yt-dlp", "ffmpeg"]),
        features: owned(&["v3_compression", "welcome_hash_ack"]),
        warp_active: Some(false),
        server_version: Some("v2".to_owned()),
        yt_dlp_version: Some("yd2".to_owned()),
        ..welcome(Some("node2"))
    };
    let writer = cache(&c);
    writer.store("us1.vrcresolver.com", &first, "hash1");
    writer.store("eu1.vrcresolver.com", &second, "hash2");
    let reopened = cache(&c);
    let a = reopened
        .get("us1.vrcresolver.com")
        .unwrap_or_else(|| panic!("us1 missing"));
    let b = reopened
        .get("eu1.vrcresolver.com")
        .unwrap_or_else(|| panic!("eu1 missing"));
    assert_eq!(a.welcome_hash.as_deref(), Some("hash1"));
    assert_eq!(a.node.as_deref(), Some("node1"));
    assert_eq!(a.warp_active, Some(true));
    assert_eq!(a.server_version.as_deref(), Some("v1"));
    assert_eq!(a.yt_dlp_version.as_deref(), Some("yd1"));
    assert!(a.stored_at.is_some());
    assert_eq!(b.welcome_hash.as_deref(), Some("hash2"));
    assert_eq!(b.engines.as_ref().map(Vec::len), Some(2));
    assert_eq!(b.warp_active, Some(false));
}

#[test]
fn welcome_cache_nodes_do_not_evict_each_other() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("us1.vrcresolver.com", &welcome(Some("node1")), "h1");
    wc.store("eu1.vrcresolver.com", &welcome(Some("node2")), "h2");
    assert_eq!(
        wc.get("us1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h1")
    );
    assert_eq!(
        wc.get("eu1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h2")
    );
}

#[test]
fn welcome_cache_lookup_ignores_ascii_case_and_restore_replaces() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("US1.vrcresolver.com", &welcome(None), "h1");
    assert_eq!(
        wc.get("us1.VRCRESOLVER.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h1")
    );
    wc.store("us1.vrcresolver.com", &welcome(None), "h2");
    let text = fs::read_to_string(&c.path).unwrap_or_default();
    assert!(!text.contains("US1."), "{text}");
    assert_eq!(
        wc.get("US1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h2")
    );
}

#[test]
fn welcome_cache_invalidate_removes_only_that_node() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("us1.vrcresolver.com", &welcome(None), "h1");
    wc.store("eu1.vrcresolver.com", &welcome(None), "h2");
    wc.invalidate("us1.vrcresolver.com");
    assert_eq!(wc.get("us1.vrcresolver.com"), None);
    assert!(wc.get("eu1.vrcresolver.com").is_some());
}

#[test]
fn welcome_cache_corrupt_file_is_a_miss() {
    let c = cache_dir();
    fs::write(&c.path, "{ this is not valid json").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cache(&c).get("us1.vrcresolver.com"), None);
}

#[test]
fn welcome_cache_null_file_is_a_miss_and_can_be_overwritten() {
    let c = cache_dir();
    fs::write(&c.path, "null").unwrap_or_else(|e| panic!("{e}"));
    let wc = cache(&c);
    assert_eq!(wc.get("n"), None);
    wc.store("n", &welcome(None), "h");
    assert!(wc.get("n").is_some());
}

#[test]
fn welcome_cache_write_leaves_no_temp_file() {
    let c = cache_dir();
    cache(&c).store("node1", &welcome(None), "h");
    assert!(c.path.exists());
    assert!(!identity::sibling(&c.path, ".new").exists());
}

#[test]
fn welcome_cache_empty_node_is_a_miss() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("", &welcome(None), "h");
    assert_eq!(wc.get(""), None);
    assert!(!c.path.exists());
}

#[test]
fn welcome_cache_empty_hash_is_not_stored() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("node1", &welcome(None), "");
    assert_eq!(wc.get("node1"), None);
    assert!(!c.path.exists());
}

#[test]
fn welcome_cache_oversized_file_is_a_miss_and_moved_aside() {
    let c = cache_dir();
    let cap = usize::try_from(welcome_cache::MAX_FILE_BYTES).unwrap_or(65_536);
    let mut bytes = vec![0u8; cap + 1];
    bytes[0] = b'{';
    fs::write(&c.path, &bytes).unwrap_or_else(|e| panic!("{e}"));
    let log_path = c.dir.join("watchdog.log");
    let wc = WelcomeCache::new(c.path.clone(), Log::to_file(&log_path));
    assert_eq!(wc.get("us1.vrcresolver.com"), None);
    assert!(!c.path.exists());
    let aside: Vec<String> = list(&c.dir)
        .into_iter()
        .filter(|n| n.starts_with("v3_welcome_cache.json.oversized-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(
        fs::metadata(c.dir.join(&aside[0])).map(|m| m.len()).unwrap_or(0),
        welcome_cache::MAX_FILE_BYTES + 1
    );
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    assert!(log.contains("[v3-cache] oversized cache file"), "{log}");
}

#[cfg(windows)]
#[test]
fn welcome_cache_failed_save_cleans_its_temp_file() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("us1.vrcresolver.com", &welcome(None), "h0");
    {
        let _lock = lock_exclusively(&c.path);
        wc.store("us1.vrcresolver.com", &welcome(None), "h1");
    }
    assert!(!identity::sibling(&c.path, ".new").exists());
    assert_eq!(
        wc.get("us1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h0")
    );
}

#[test]
fn welcome_cache_keeps_the_welcome_hosts_fields() {
    let c = cache_dir();
    let wc = cache(&c);
    let frame = WelcomeFrame {
        features: owned(&[feature::WELCOME_HOSTS]),
        first_party_hosts: owned(&["vrcresolver.com", "whyknot.dev"]),
        playback_proxy_paths: owned(&["/api/proxy", "/api/popcorn/proxy"]),
        ..welcome(Some("node1"))
    };
    wc.store("us1.vrcresolver.com", &frame, "hash1");
    let entry = wc
        .get("us1.vrcresolver.com")
        .unwrap_or_else(|| panic!("entry missing"));
    assert_eq!(
        entry.first_party_hosts,
        owned(&["vrcresolver.com", "whyknot.dev"])
    );
    assert_eq!(
        entry.playback_proxy_paths,
        owned(&["/api/proxy", "/api/popcorn/proxy"])
    );
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
        keep.lock().unwrap_or_else(|e| e.into_inner()).push(p.clone());
        Ok(status)
    });
    (post, sent)
}

fn reporter(enabled: bool, post: Post) -> (Reporter, Capture) {
    let out = Capture::default();
    let ux = Ux::new(Box::new(out.clone()), Style::plain(), String::new);
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

#[test]
fn failure_kinds_follow_the_old_mapping() {
    assert_eq!(
        failure_kind(fallback::ALL_CONFIGS_FAILED),
        Some("AllStrategiesFailed")
    );
    assert_eq!(failure_kind(fallback::EXTRACTOR_UNSUPPORTED), Some("Unknown"));
    assert_eq!(failure_kind(fallback::INTERNAL_ERROR), Some("Unknown"));
    assert_eq!(failure_kind(fallback::UNITY_UNSUPPORTED_FORMAT), Some("Unknown"));
    assert_eq!(failure_kind(fallback::WARP_DOWN), Some("NetworkError"));
    for skipped in [
        fallback::DISCOVERY_IN_PROGRESS,
        fallback::SERVER_UNREACHABLE,
        fallback::CLIENT_DEADLINE_EXCEEDED,
        fallback::RATE_LIMITED,
        "",
    ] {
        assert_eq!(failure_kind(skipped), None, "{skipped}");
    }
}

#[test]
fn hash_short_is_the_first_six_sha256_bytes() {
    assert_eq!(hash_short(""), "");
    assert_eq!(hash_short("abc"), "ba7816bf8f01");
}

#[test]
fn url_parts_for_reports() {
    assert_eq!(
        extract_domain("https://www.YouTube.com/watch?v=x").as_deref(),
        Some("youtube.com")
    );
    assert_eq!(
        extract_domain("https://m.twitch.tv/x").as_deref(),
        Some("m.twitch.tv")
    );
    assert_eq!(extract_domain("not a url"), None);
    assert_eq!(path_and_query("https://a.b/watch?v=x&t=5"), "/watch?v=x&t=5");
    assert_eq!(path_and_query("https://a.b/clip"), "/clip");
    assert_eq!(path_and_query("::"), "");
}

#[test]
fn replace_ignore_ascii_case_replaces_every_match() {
    assert_eq!(
        replace_ignore_ascii_case("Alice met ALICE", "alice", "<u>"),
        "<u> met <u>"
    );
    assert_eq!(replace_ignore_ascii_case("abc", "", "x"), "abc");
    assert_eq!(
        replace_ignore_ascii_case("caf\u{e9} alice", "ALICE", "<u>"),
        "caf\u{e9} <u>"
    );
}

#[test]
fn sanitize_scrubs_identity_paths_ips_and_tokens() {
    let (post, _) = recording_post(200);
    let (r, _) = reporter(true, post);
    let url = "https://youtu.be/dQw4w9WgXcQ";
    let raw =
        format!("C:\\Users\\alice\\x.txt on DESKTOP-1 at 10.0.0.5 key abcdefghijklmnopqrstuvwxyz for {url}");
    assert_eq!(
        r.sanitize(&raw, Some(url)),
        "<path> on <machine> at <ip> key <token> for <url>"
    );
    assert_eq!(r.sanitize("/home/bob/.cache/x failed", None), "<path> failed");
    assert_eq!(r.sanitize("", Some(url)), "");
}

#[test]
fn sanitize_leaves_short_user_names_alone() {
    let (post, _) = recording_post(200);
    let out = Capture::default();
    let ux = Ux::new(Box::new(out), Style::plain(), String::new);
    let short = Machine {
        user_name: Some("al".to_owned()),
        user_profile: None,
        machine_name: None,
    };
    let r = Reporter::new(true, "1", ux, short, post);
    assert_eq!(r.sanitize("al was here", None), "al was here");
}

#[test]
fn reports_are_limited_per_session_and_spaced_out() {
    let (post, _) = recording_post(200);
    let (r, _) = reporter(true, post);
    let t0 = std::time::Instant::now();
    assert!(r.take_slot(t0));
    assert!(!r.take_slot(t0 + MIN_INTERVAL - Duration::from_secs(1)));
    for i in 1..MAX_PER_SESSION {
        assert!(r.take_slot(t0 + MIN_INTERVAL * i), "slot {i}");
    }
    assert!(!r.take_slot(t0 + MIN_INTERVAL * (MAX_PER_SESSION + 5)));
}

#[test]
fn build_hashes_the_url_and_scrubs_the_summary() {
    let (post, _) = recording_post(200);
    let (r, _) = reporter(true, post);
    let req = yt_request();
    let summary = format!("yt-dlp failed for {} as alice", req.url);
    let p = r
        .build(&req, "AllStrategiesFailed", Some(&summary))
        .unwrap_or_else(|| panic!("no payload"));
    assert_eq!(p.app_version, "1.2.3");
    assert_eq!(p.failure_kind, "AllStrategiesFailed");
    assert_eq!(p.url_domain, "youtube.com");
    assert_eq!(p.player, "unity");
    assert_eq!(p.stream_type, "vod");
    assert_eq!(p.url_path_hash_short, hash_short("/watch?v=dQw4w9WgXcQ&t=5"));
    assert_eq!(p.video_id_hash_short, hash_short("dQw4w9WgXcQ"));
    assert_eq!(p.error_summary, "yt-dlp failed for <url> as <user>");
    let long = "x ".repeat(MAX_SUMMARY_CHARS);
    let p = r
        .build(
            &ResolveRequest {
                player: None,
                ..yt_request()
            },
            "Unknown",
            Some(&long),
        )
        .unwrap_or_else(|| panic!("no payload"));
    assert_eq!(p.player, "avpro");
    assert_eq!(p.error_summary.chars().count(), MAX_SUMMARY_CHARS);
    let bad = ResolveRequest {
        url: "not a url".to_owned(),
        ..yt_request()
    };
    assert!(r.build(&bad, "Unknown", None).is_none());
}

#[test]
fn youtube_ids_are_found_in_the_usual_shapes() {
    let (post, _) = recording_post(200);
    let (r, _) = reporter(true, post);
    for url in [
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        "https://youtu.be/dQw4w9WgXcQ?t=3",
        "https://www.youtube.com/embed/dQw4w9WgXcQ",
        "https://www.youtube.com/live/dQw4w9WgXcQ",
    ] {
        assert_eq!(r.youtube_id(url).as_deref(), Some("dQw4w9WgXcQ"), "{url}");
    }
    assert_eq!(r.youtube_id("https://twitch.tv/x"), None);
}

fn wait_for(sent: &Sent, count: usize) {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while sent.lock().unwrap_or_else(|e| e.into_inner()).len() < count {
        assert!(std::time::Instant::now() < until, "report never posted");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn report_fallback_posts_once_for_a_reportable_reason() {
    let (post, sent) = recording_post(200);
    let (r, _) = reporter(true, post);
    let r = Arc::new(r);
    r.report_fallback(&yt_request(), fallback::DISCOVERY_IN_PROGRESS, None);
    r.report_fallback(
        &ResolveRequest {
            url: String::new(),
            ..yt_request()
        },
        fallback::ALL_CONFIGS_FAILED,
        None,
    );
    r.report_fallback(&yt_request(), fallback::ALL_CONFIGS_FAILED, Some("boom"));
    wait_for(&sent, 1);
    r.report_fallback(&yt_request(), fallback::ALL_CONFIGS_FAILED, Some("again"));
    std::thread::sleep(Duration::from_millis(100));
    let got = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].error_summary, "boom");
}

#[test]
fn disabled_reporter_never_posts() {
    let calls = Arc::new(AtomicU32::new(0));
    let seen = Arc::clone(&calls);
    let post: Post = Arc::new(move |_: &ReportPayload| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(200)
    });
    let (r, _) = reporter(false, post);
    let r = Arc::new(r);
    assert!(!r.enabled());
    r.report_fallback(&yt_request(), fallback::ALL_CONFIGS_FAILED, None);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn rejected_and_failed_posts_are_warned_about() {
    let (post, _) = recording_post(429);
    let (r, out) = reporter(true, post);
    let payload = r
        .build(&yt_request(), "Unknown", None)
        .unwrap_or_else(|| panic!("no payload"));
    r.send(&payload);
    assert!(
        out.text().contains("server rejected report: 429"),
        "{}",
        out.text()
    );
    let failing: Post = Arc::new(|_: &ReportPayload| Err("dns failure".to_owned()));
    let (r, out) = reporter(true, failing);
    r.send(&payload);
    assert!(out.text().contains("post failed: dns failure"), "{}", out.text());
}

#[test]
fn contract_subprotocols_match() {
    let c = contract();
    let current = c["subprotocols"]["current"].as_str().unwrap_or_default();
    let legacy = c["subprotocols"]["legacy"].as_str().unwrap_or_default();
    assert_eq!(current, SUBPROTOCOL_V3);
    assert_eq!(
        c["protocol_version"].as_i64(),
        Some(i64::from(CLIENT_PROTOCOL_VERSION))
    );
    assert!(should_send_client_hello(Some(current)));
    assert!(!should_send_client_hello(Some(legacy)));
}

#[test]
fn contract_client_fallback_reasons_exist_on_the_server() {
    let server = strings(&contract(), "fallback_reasons");
    for reason in [
        fallback::ALL_CONFIGS_FAILED,
        fallback::EXTRACTOR_UNSUPPORTED,
        fallback::INTERNAL_ERROR,
        fallback::DISCOVERY_IN_PROGRESS,
        fallback::UNITY_UNSUPPORTED_FORMAT,
        fallback::WARP_DOWN,
    ] {
        assert!(
            server.iter().any(|r| r == reason),
            "{reason} missing from the server list"
        );
    }
}

#[test]
fn contract_playback_feedback_kinds_match() {
    assert_eq!(
        strings(&contract(), "playback_feedback_kinds"),
        sorted(&[
            feedback::LOAD_FAILURE,
            feedback::SILENT_STALL,
            feedback::PLAYING,
            feedback::RESOLVED_REJECTED,
            feedback::OG_FAILED,
            feedback::CACHE_PLAY,
        ])
    );
}

#[test]
fn contract_resolved_rejected_details_match() {
    assert_eq!(
        strings(&contract(), "resolved_rejected_details"),
        sorted(&[og_reason::AVPRO_INCOMPATIBLE, og_reason::RESOLVED_URL_REJECTED])
    );
}

#[test]
fn contract_og_failed_details_match() {
    assert_eq!(
        strings(&contract(), "og_failed_details"),
        sorted(&OG_FAILED_DETAILS)
    );
    for detail in OG_FAILED_DETAILS {
        assert_eq!(og_failed_detail_for(Some(detail)), detail);
    }
    assert_eq!(
        og_failed_detail_for(Some("something_else")),
        OG_FAILED_DETAIL_UNKNOWN
    );
}

#[test]
fn contract_proxy_extensions_match() {
    assert_eq!(
        strings(&contract(), "proxy_path_extensions"),
        sorted(&ALLOWED_PATH_EXTENSIONS)
    );
}

#[test]
fn contract_client_needed_features_are_listed_by_the_server() {
    let c = contract();
    let needed = [
        feature::PLAYBACK_FEEDBACK,
        feature::PLAYBACK_FEEDBACK_V2,
        feature::WELCOME_HOSTS,
    ];
    assert_eq!(strings(&c, "client_required_features"), sorted(&needed));
    let server = strings(&c, "server_features");
    let welcome: WelcomeFrame =
        serde_json::from_value(json_sample(&c, action::WELCOME)).unwrap_or_else(|e| panic!("{e}"));
    let advertised = welcome.features.unwrap_or_default();
    for f in needed {
        assert!(server.iter().any(|s| s == f), "{f} not in server_features");
        assert!(advertised.iter().any(|s| s == f), "{f} not in the welcome sample");
    }
}

#[test]
fn contract_msgpack_samples_match_the_json_samples() {
    let c = contract();
    let names: Vec<String> = c["msgpack_samples_hex"]
        .as_object()
        .map(|o| {
            let mut v: Vec<String> = o.keys().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        sorted(&[action::RESOLVED, action::FALLBACK_NATIVE, action::RESOLVE_LOG])
    );

    let want = json_sample(&c, action::RESOLVED);
    let got = resolved(&msgpack_sample(&c, action::RESOLVED));
    assert_eq!(Some(got.action.clone()), text(&want, "action"));
    assert_eq!(Some(got.id.clone()), text(&want, "id"));
    assert_eq!(got.url, text(&want, "url"));
    assert_eq!(got.engine, text(&want, "engine"));
    assert_eq!(got.config, text(&want, "config"));
    assert_eq!(got.container, text(&want, "container"));
    assert_eq!(got.video_codec, text(&want, "video_codec"));
    assert_eq!(got.audio_codec, text(&want, "audio_codec"));
    assert_eq!(got.protocol, text(&want, "protocol"));
    assert_eq!(got.audio_channels.map(i64::from), int(&want, "audio_channels"));
    assert_eq!(got.bytes_estimate, int(&want, "bytes_estimate"));
    assert_eq!(got.expires_at, text(&want, "expires_at"));
    assert_eq!(got.resolved_height.map(i64::from), int(&want, "resolved_height"));
    assert!(got.resolved_height.is_some());

    let want = json_sample(&c, action::FALLBACK_NATIVE);
    let got = fallback_frame(&msgpack_sample(&c, action::FALLBACK_NATIVE));
    assert_eq!(Some(got.action.clone()), text(&want, "action"));
    assert_eq!(Some(got.id.clone()), text(&want, "id"));
    assert_eq!(got.reason, text(&want, "reason"));
    assert_eq!(got.public_message, text(&want, "public_message"));
    assert_eq!(got.retry_after_ms.map(i64::from), int(&want, "retry_after_ms"));
    assert!(got.retry_after_ms.is_some());

    let want = json_sample(&c, action::RESOLVE_LOG);
    match decode_binary(&msgpack_sample(&c, action::RESOLVE_LOG)) {
        Ok(BinaryFrame::ResolveLog { id, message }) => {
            assert_eq!(Some(id), text(&want, "id"));
            assert_eq!(Some(message), text(&want, "message"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn contract_deadlines_nest_with_the_server_window() {
    let c = contract();
    let margin = number(&c, "resolve_window", "deadline_margin_ms");
    let min_window = number(&c, "resolve_window", "min_ms");
    let max_window = number(&c, "resolve_window", "max_ms");
    let now = Instant::now();
    let mut violations = Vec::new();
    for budget in (2_000..=28_000).step_by(500) {
        let sent = budget - 1_000;
        let watchdog = effective_budget_ms(sent);
        let wire = wire_deadline_for(
            now + Duration::from_millis(u64::try_from(watchdog).unwrap_or(0)),
            now,
        );
        let server = (wire - margin).clamp(min_window, max_window);
        let line = format!("budget={budget} sent={sent} watchdog={watchdog} wire={wire} server={server}");
        if watchdog >= budget {
            violations.push(format!("watchdog does not finish before the wrapper: {line}"));
        }
        if budget >= 7_000 && server >= watchdog {
            violations.push(format!(
                "server window does not finish before the watchdog: {line}"
            ));
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn contract_fixture_is_a_copy_of_the_server_fixture() {
    let server = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../../../Deployed/vrcresolver-server/apex/crates/vrcr-protocol/tests/fixtures/mesh-contract.json",
    );
    let Ok(server_copy) = fs::read_to_string(&server) else {
        return;
    };
    assert_eq!(server_copy.replace("\r\n", "\n"), CONTRACT.replace("\r\n", "\n"));
}

#[test]
fn contract_wire_constants_keep_their_names() {
    assert_eq!(wire::SUBPROTOCOL_V3, "vrcresolver-v3");
    assert_eq!(action::CLIENT_HELLO, "client_hello");
    assert_eq!(action::WELCOME_CACHED, "welcome_cached");
    assert_eq!(action::PROTOCOL_ERROR, "protocol_error");
    assert_eq!(action::RATE_LIMITED, "rate_limited");
    assert_eq!(action::PLAYBACK_QUALITY_UPDATE, "playback_quality_update");
    assert_eq!(fallback::RATE_LIMITED, "rate_limited");
    assert_eq!(fallback::PROTOCOL_ERROR, "protocol_error");
    assert_eq!(wire::field::RETRY_AFTER_SECONDS, "retryAfterSeconds");
    assert_eq!(wire::field::MESH_ACTION, "meshAction");
    assert_eq!(wire::format::PREFERENCE, ["msgpack", "json"]);
}

#[test]
fn resolve_request_defaults_to_the_resolve_action() {
    assert_eq!(ResolveRequest::default().action, action::RESOLVE);
    let parsed: ResolveRequest =
        serde_json::from_value(json!({"id":"a","url":"https://x"})).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(parsed.action, action::RESOLVE);
    let parsed: ResolveRequest =
        serde_json::from_value(json!({"action":"other","id":"a"})).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(parsed.action, "other");
}
