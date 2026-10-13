use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::time::Instant;

use vrcr_shared::console::{Style, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::wire::{ResolveRequest, ResolveResponse, WelcomeFrame, action, fallback, feature};

use super::{Config, MeshClient, PROXY_HOST, RECENT_CIDS_TTL};
use crate::ipc::{Mesh, MeshResult};
use crate::mesh::welcome_cache::WelcomeCache;
use crate::stats::Stats;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

struct Harness {
    client: MeshClient,
    out: Capture,
    log_path: std::path::PathBuf,
    dir: TempDir,
}

impl Harness {
    fn new() -> Harness {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let out = Capture::default();
        let ux = Ux::new(Box::new(out.clone()), Style::plain(), String::new);
        let log_path = dir.path().join("watchdog.log");
        let cfg = Config::new(dir.path(), "test");
        let client = MeshClient::new(
            cfg,
            ux,
            Log::to_file(&log_path),
            Arc::new(Stats::new(SystemTime::now())),
        );
        Harness {
            client,
            out,
            log_path,
            dir,
        }
    }

    fn v3(cache_has_node: bool) -> Harness {
        let h = Harness::new();
        if cache_has_node {
            seed_cache(&h.client.inner.cfg.welcome_cache_path);
        }
        {
            let mut s = super::lock(&h.client.inner.server);
            s.is_v3 = true;
            s.current_host = PROXY_HOST.to_owned();
        }
        h
    }

    async fn json(&self, text: &str) {
        self.client.inner.dispatch_text(None, text.as_bytes()).await;
    }

    async fn binary(&self, payload: &[u8]) {
        self.client.inner.dispatch_binary(None, payload).await;
    }

    fn pending(&self, id: &str) -> oneshot::Receiver<MeshResult> {
        let (tx, rx) = oneshot::channel();
        super::lock(&self.client.inner.pending).insert(id.to_owned(), tx);
        rx
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    fn cooldown(&self) -> Option<Instant> {
        *super::lock(&self.client.inner.rate_limited_until)
    }
}

fn seed_cache(path: &Path) {
    let cache = WelcomeCache::new(path.to_path_buf(), Log::disabled());
    let welcome = WelcomeFrame {
        protocol_version: 3,
        node: Some("node1".to_owned()),
        features: Some(vec![
            feature::PLAYBACK_FEEDBACK.to_owned(),
            feature::WELCOME_HOSTS.to_owned(),
        ]),
        first_party_hosts: Some(vec!["example.org".to_owned()]),
        server_version: Some("s1".to_owned()),
        ..WelcomeFrame::default()
    };
    cache.store(PROXY_HOST, &welcome, "hash1");
}

fn mp_frame(items: Vec<rmpv::Value>) -> Vec<u8> {
    let mut out = Vec::new();
    rmpv::encode::write_value(&mut out, &rmpv::Value::Array(items)).unwrap_or_else(|e| panic!("{e}"));
    out
}

fn mp_str(s: &str) -> rmpv::Value {
    rmpv::Value::from(s)
}

fn done(rx: &mut oneshot::Receiver<MeshResult>) -> Option<MeshResult> {
    rx.try_recv().ok()
}

#[tokio::test]
async fn rate_limited_resolve_with_id_fails_that_pending_only() {
    let h = Harness::new();
    let mut limited = h.pending("req1");
    let mut other = h.pending("req2");

    h.json(r#"{"action":"rate_limited","meshAction":"resolve","limit":10,"windowSeconds":60,"retryAfterSeconds":6,"id":"req1"}"#)
        .await;

    let result = done(&mut limited).unwrap_or_else(|| panic!("req1 still pending"));
    assert_eq!(result.reason.as_deref(), Some(fallback::RATE_LIMITED));
    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert!(done(&mut other).is_none());
    assert!(h.cooldown().is_some_and(|t| t > Instant::now()));
}

#[tokio::test]
async fn rate_limited_resolve_without_id_fails_all_pending() {
    let h = Harness::new();
    let mut a = h.pending("a");
    let mut b = h.pending("b");

    h.json(r#"{"action":"rate_limited","meshAction":"resolve","limit":10,"windowSeconds":60,"retryAfterSeconds":6}"#)
        .await;

    let a = done(&mut a).unwrap_or_else(|| panic!("a still pending"));
    assert!(done(&mut b).is_some());
    assert_eq!(a.reason.as_deref(), Some(fallback::RATE_LIMITED));
    assert!(
        h.out
            .text()
            .contains("failing 2 pending requests reason=rate_limited ids=a,b")
    );
}

#[tokio::test]
async fn rate_limited_cooldown_is_clamped_to_the_ceiling() {
    let h = Harness::new();

    h.json(r#"{"action":"rate_limited","meshAction":"resolve","limit":10,"windowSeconds":60,"retryAfterSeconds":86400}"#)
        .await;

    let until = h.cooldown().unwrap_or_else(|| panic!("no cooldown"));
    assert!(until <= Instant::now() + Duration::from_secs(61));
    assert!(until > Instant::now());
}

#[tokio::test]
async fn rate_limited_non_resolve_action_sets_no_cooldown() {
    let h = Harness::new();

    h.json(r#"{"action":"rate_limited","meshAction":"playback_feedback","limit":30,"windowSeconds":60,"retryAfterSeconds":2}"#)
        .await;

    assert!(h.cooldown().is_none());
    assert!(
        h.out
            .text()
            .contains("server rate-limited playback_feedback -- pausing for 2s")
    );
}

#[tokio::test]
async fn resolve_short_circuits_during_the_cooldown_without_wire_traffic() {
    let h = Harness::new();
    *super::lock(&h.client.inner.rate_limited_until) = Some(Instant::now() + Duration::from_secs(30));
    let req = ResolveRequest {
        url: "https://example.com/v".to_owned(),
        ..ResolveRequest::default()
    };

    let result = h
        .client
        .resolve(&req, Instant::now() + Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert_eq!(result.reason.as_deref(), Some(fallback::RATE_LIMITED));
    let frame: ResolveResponse = serde_json::from_slice(&result.frame).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(frame.reason.as_deref(), Some(fallback::RATE_LIMITED));
    assert_eq!(frame.id.len(), 32);
    assert!(super::lock(&h.client.inner.pending).is_empty());
}

#[tokio::test]
async fn protocol_error_with_id_fails_that_pending() {
    let h = Harness::new();
    let mut rx = h.pending("bad1");

    h.json(r#"{"action":"protocol_error","reason":"oversize_field","field":"url","id":"bad1"}"#)
        .await;

    let result = done(&mut rx).unwrap_or_else(|| panic!("still pending"));
    assert_eq!(result.reason.as_deref(), Some(fallback::PROTOCOL_ERROR));
    assert!(
        h.out
            .text()
            .contains("server protocol_error reason=oversize_field field=url id=bad1")
    );
}

#[tokio::test]
async fn protocol_error_without_id_never_touches_pending() {
    let h = Harness::new();
    let mut rx = h.pending("r1");

    h.json(r#"{"action":"protocol_error","reason":"invalid_field","field":"kind"}"#)
        .await;

    assert!(done(&mut rx).is_none());
}

#[tokio::test]
async fn cached_welcome_alone_keeps_the_reconnect_backoff() {
    let h = Harness::v3(true);
    h.client
        .inner
        .reconnect_attempt
        .store(4, std::sync::atomic::Ordering::SeqCst);

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"node":"node1"}"#)
        .await;

    assert_eq!(h.client.reconnect_attempt(), 4);
    assert_eq!(h.client.server_protocol_version(), 3);
}

#[tokio::test]
async fn full_welcome_alone_keeps_the_reconnect_backoff() {
    let h = Harness::v3(false);
    h.client
        .inner
        .reconnect_attempt
        .store(4, std::sync::atomic::Ordering::SeqCst);

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1"}"#)
        .await;

    assert_eq!(h.client.reconnect_attempt(), 4);
}

#[tokio::test]
async fn cached_welcome_without_a_local_entry_keeps_the_backoff() {
    let h = Harness::v3(false);
    h.client
        .inner
        .reconnect_attempt
        .store(4, std::sync::atomic::Ordering::SeqCst);

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"node":"node1"}"#)
        .await;

    assert_eq!(h.client.reconnect_attempt(), 4);
    assert!(
        h.out
            .text()
            .contains("welcome_cached but local entry missing -- invalidating + reconnecting")
    );
}

#[tokio::test]
async fn resolved_frame_resets_the_backoff_and_completes_its_request() {
    let h = Harness::new();
    h.client
        .inner
        .reconnect_attempt
        .store(4, std::sync::atomic::Ordering::SeqCst);
    let mut rx = h.pending("r1");

    h.json(r#"{"action":"resolved","id":"r1","url":"https://media.example/v.mp4","bytes_estimate":1234}"#)
        .await;

    let result = done(&mut rx).unwrap_or_else(|| panic!("still pending"));
    assert_eq!(result.action, action::RESOLVED);
    assert_eq!(h.client.reconnect_attempt(), 0);
    let frame: Value = serde_json::from_slice(&result.frame).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(frame["url"], "https://media.example/v.mp4");
}

#[tokio::test]
async fn fallback_frame_carries_reason_and_retry_hint_and_is_logged() {
    let h = Harness::new();
    let mut rx = h.pending("r1");

    h.json(r#"{"action":"fallback_native","id":"r1","reason":"discovery_in_progress","retry_after_ms":800}"#)
        .await;

    let result = done(&mut rx).unwrap_or_else(|| panic!("still pending"));
    assert_eq!(result.reason.as_deref(), Some(fallback::DISCOVERY_IN_PROGRESS));
    assert_eq!(result.retry_after_ms, Some(800));
    assert!(
        h.log()
            .contains("[mesh] fallback_native id=r1 reason=discovery_in_progress")
    );
    assert_eq!(h.out.text(), "");
}

#[tokio::test]
async fn resolved_url_remembers_the_correlation_id_for_later_feedback() {
    let h = Harness::new();
    let _rx = h.pending("r1");
    super::lock(&h.client.inner.inflight_cids).insert("r1".to_owned(), "cid-1".to_owned());

    h.json(r#"{"action":"resolved","id":"r1","url":"https://media.example/v.mp4"}"#)
        .await;

    assert_eq!(
        h.client
            .inner
            .lookup_recent_cid("https://media.example/v.mp4")
            .as_deref(),
        Some("cid-1")
    );
    assert!(super::lock(&h.client.inner.inflight_cids).is_empty());
}

#[tokio::test]
async fn recent_correlation_ids_are_capped_and_expire() {
    let h = Harness::new();
    let inner = &h.client.inner;
    for i in 0..=super::MAX_RECENT_CIDS {
        inner.remember_resolved_url_cid(&format!("https://m.example/{i}"), &format!("c{i}"));
    }

    assert_eq!(super::lock(&inner.recent_cids).len(), super::MAX_RECENT_CIDS);
    assert!(inner.lookup_recent_cid("https://m.example/0").is_none());
    assert_eq!(
        inner
            .lookup_recent_cid(&format!("https://m.example/{}", super::MAX_RECENT_CIDS))
            .as_deref(),
        Some(format!("c{}", super::MAX_RECENT_CIDS).as_str())
    );

    if let Some(old) = Instant::now().checked_sub(RECENT_CIDS_TTL + Duration::from_secs(1)) {
        super::lock(&inner.recent_cids).insert("https://m.example/old".to_owned(), ("c-old".to_owned(), old));
        assert!(inner.lookup_recent_cid("https://m.example/old").is_none());
    }
}

#[tokio::test]
async fn failing_many_pending_requests_lists_only_the_first_eight_ids() {
    let h = Harness::new();
    let mut receivers = Vec::new();
    for i in 0..10 {
        receivers.push(h.pending(&format!("id{i:02}")));
    }

    h.client.inner.fail_all_pending(fallback::SERVER_UNREACHABLE);

    assert!(receivers.iter_mut().all(|rx| done(rx).is_some()));
    assert!(h.out.text().contains(
        "failing 10 pending requests reason=server_unreachable ids=id00,id01,id02,id03,id04,id05,id06,id07,...(+2)"
    ));
}

#[tokio::test]
async fn welcome_records_the_server_and_applies_its_hosts() {
    let h = Harness::new();

    h.json(
        r#"{"action":"welcome","protocol_version":9,"node":"node1","server_version":"s1","yt_dlp_version":"y1","warp_active":true,"features":["welcome_hosts"],"first_party_hosts":["example.org"],"playback_proxy_paths":["/p"]}"#,
    )
    .await;

    assert_eq!(h.client.server_protocol_version(), 3);
    assert_eq!(h.client.server_node().as_deref(), Some("node1"));
    assert_eq!(h.client.server_version().as_deref(), Some("s1"));
    assert_eq!(h.client.yt_dlp_version().as_deref(), Some("y1"));
    assert_eq!(h.client.warp_active(), Some(true));
    let policy = h.client.first_party();
    let policy = policy.read().unwrap_or_else(|e| e.into_inner());
    assert!(policy.host_families().iter().any(|f| f == "example.org"));
    assert!(h.out.text().contains(
        "welcome node=node1 v=9 (negotiated=3) server=s1 yt-dlp=y1 warp_active=true features=[welcome_hosts]"
    ));
}

#[tokio::test]
async fn welcome_without_features_warns_and_keeps_the_default_hosts() {
    let h = Harness::new();
    let before = h
        .client
        .first_party()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .host_families()
        .to_vec();

    h.json(r#"{"action":"welcome","protocol_version":1,"node":"fake","first_party_hosts":["evil.example"]}"#)
        .await;

    assert!(h.out.text().contains("welcome missing required field: features"));
    let after = h
        .client
        .first_party()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .host_families()
        .to_vec();
    assert_eq!(before, after);
}

#[tokio::test]
async fn v3_welcome_with_a_hash_is_cached_for_the_next_connect() {
    let h = Harness::v3(false);

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h9"}"#)
        .await;

    let entry = h
        .client
        .inner
        .welcome_cache
        .get(PROXY_HOST)
        .unwrap_or_else(|| panic!("not cached"));
    assert_eq!(entry.welcome_hash.as_deref(), Some("h9"));
}

#[tokio::test]
async fn welcome_on_a_non_v3_link_is_not_cached() {
    let h = Harness::new();
    super::lock(&h.client.inner.server).current_host = PROXY_HOST.to_owned();

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h9"}"#)
        .await;

    assert!(h.client.inner.welcome_cache.get(PROXY_HOST).is_none());
}

#[tokio::test]
async fn cached_welcome_hydrates_the_server_from_the_local_entry() {
    let h = Harness::v3(true);

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"warp_active":true,"negotiated_format":"msgpack"}"#)
        .await;

    assert_eq!(h.client.server_node().as_deref(), Some("node1"));
    assert_eq!(h.client.server_version().as_deref(), Some("s1"));
    assert_eq!(h.client.warp_active(), Some(true));
    assert_eq!(h.client.negotiated_format(), "msgpack");
    assert!(h.client.inner.is_msgpack());
    let policy = h.client.first_party();
    assert!(
        policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .host_families()
            .iter()
            .any(|f| f == "example.org")
    );
    assert!(
        h.log()
            .contains("[mesh][v3] welcome_cached hit node=node1 v=3 negotiated_format=msgpack")
    );
}

#[tokio::test]
async fn cached_welcome_on_a_non_v3_link_is_a_protocol_error() {
    let h = Harness::new();

    h.json(r#"{"action":"welcome_cached","protocol_version":3}"#)
        .await;

    assert!(
        h.out
            .text()
            .contains("welcome_cached received on non-v3 connection")
    );
    assert_eq!(h.client.server_protocol_version(), 0);
}

#[tokio::test]
async fn unknown_action_is_discarded_with_a_warning() {
    let h = Harness::new();

    h.json(r#"{"action":"frobnicate"}"#).await;

    assert!(h.out.text().contains("unknown action -- discarding: frobnicate"));
}

#[tokio::test]
async fn repeated_parse_failures_warn_once_a_minute() {
    let h = Harness::new();

    h.json("{not json").await;
    h.json("{still not json").await;

    let text = h.out.text();
    assert_eq!(text.matches("frame parse failed").count(), 1, "{text}");
    assert!(text.contains("frame parse failed (json x1 in last min)"));
}

#[tokio::test]
async fn playback_quality_update_is_not_reported_as_unknown() {
    let h = Harness::new();

    h.json(r#"{"action":"playback_quality_update","correlation_id":"c1","delivered_height":1080,"resolved_height":1080}"#)
        .await;

    assert_eq!(h.out.text(), "");
    assert!(
        h.log()
            .contains("[mesh] playback quality cid=c1 delivered=1080p resolved=1080p")
    );
}

#[tokio::test]
async fn playback_quality_update_with_missing_or_odd_fields_stays_quiet() {
    for json in [
        r#"{"action":"playback_quality_update","correlation_id":"c1","delivered_height":720}"#,
        r#"{"action":"playback_quality_update","correlation_id":"c1","resolved_height":1080}"#,
        r#"{"action":"playback_quality_update","correlation_id":"c1","delivered_height":"tall","resolved_height":null}"#,
        r#"{"action":"playback_quality_update","correlation_id":7,"delivered_height":1080.5,"resolved_height":99999999999}"#,
        r#"{"action":"playback_quality_update"}"#,
    ] {
        let h = Harness::new();
        h.json(json).await;
        assert_eq!(h.out.text(), "", "{json}");
    }
}

#[tokio::test]
async fn resolve_log_goes_to_the_file_only() {
    let h = Harness::new();

    h.json(r#"{"action":"resolve_log","id":"r1","level":"warn","message":"trying tv"}"#)
        .await;

    assert_eq!(h.out.text(), "");
    assert!(h.log().contains("[mesh][resolve_log] id=r1 level=warn trying tv"));
}

#[tokio::test]
async fn text_pong_advances_the_last_pong() {
    let h = Harness::new();
    assert!(super::lock(&h.client.inner.last_pong).is_none());

    h.json(r#"{"action":"pong"}"#).await;

    assert!(super::lock(&h.client.inner.last_pong).is_some());
}

#[tokio::test]
async fn binary_pong_advances_the_last_pong() {
    let h = Harness::new();
    let before = Instant::now();

    h.binary(&mp_frame(vec![mp_str(action::PONG)])).await;

    assert!(super::lock(&h.client.inner.last_pong).is_some_and(|t| t >= before));
    assert!(h.log().contains("pong received via binary path"));
}

#[tokio::test]
async fn binary_ping_without_a_link_is_harmless() {
    let h = Harness::new();

    h.binary(&mp_frame(vec![mp_str(action::PING)])).await;

    assert!(h.log().contains("ping received via binary path"));
}

#[tokio::test]
async fn binary_unknown_action_falls_through_to_the_default() {
    let h = Harness::new();

    h.binary(&mp_frame(vec![mp_str("frobnicate_widget")])).await;

    assert!(
        h.out
            .text()
            .contains("unknown binary action -- discarding: frobnicate_widget")
    );
}

#[tokio::test]
async fn binary_control_frames_that_belong_on_text_are_flagged() {
    let h = Harness::new();

    h.binary(&mp_frame(vec![mp_str(action::RATE_LIMITED)])).await;

    assert!(h.out.text().contains("rate_limited received via binary path"));
}

#[tokio::test]
async fn binary_resolved_completes_its_request_with_a_json_frame() {
    let h = Harness::new();
    let mut rx = h.pending("r1");
    let nil = rmpv::Value::Nil;

    h.binary(&mp_frame(vec![
        mp_str(action::RESOLVED),
        mp_str("r1"),
        mp_str("https://media.example/v.mp4"),
        mp_str("yt-dlp"),
        nil.clone(),
        mp_str("mp4"),
        nil.clone(),
        nil.clone(),
        mp_str("http"),
        rmpv::Value::from(2),
        rmpv::Value::from(99),
        nil,
        rmpv::Value::from(1080),
    ]))
    .await;

    let result = done(&mut rx).unwrap_or_else(|| panic!("still pending"));
    assert_eq!(result.action, action::RESOLVED);
    let frame: ResolveResponse = serde_json::from_slice(&result.frame).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(frame.url.as_deref(), Some("https://media.example/v.mp4"));
    assert_eq!(frame.container.as_deref(), Some("mp4"));
    assert_eq!(frame.resolved_height, Some(1080));
    assert_eq!(frame.bytes_estimate, Some(99));
}

#[tokio::test]
async fn binary_fallback_completes_its_request_with_the_reason() {
    let h = Harness::new();
    let mut rx = h.pending("r1");

    h.binary(&mp_frame(vec![
        mp_str(action::FALLBACK_NATIVE),
        mp_str("r1"),
        mp_str(fallback::WARP_DOWN),
        rmpv::Value::Nil,
        rmpv::Value::from(500),
    ]))
    .await;

    let result = done(&mut rx).unwrap_or_else(|| panic!("still pending"));
    assert_eq!(result.reason.as_deref(), Some(fallback::WARP_DOWN));
    assert_eq!(result.retry_after_ms, Some(500));
    assert!(h.log().contains("reason=warp_down (server WARP egress unhealthy"));
}

#[tokio::test]
async fn broken_binary_frames_warn_once_per_kind_a_minute() {
    let h = Harness::new();

    h.binary(&[0xC1]).await;
    h.binary(&[0xC1]).await;
    h.binary(&mp_frame(vec![])).await;

    let text = h.out.text();
    assert_eq!(
        text.matches("binary frame parse failed (binary:action peek")
            .count(),
        1,
        "{text}"
    );
    assert!(text.contains("empty msgpack array (count=0)"));
}

#[tokio::test]
async fn resolve_during_a_long_outage_fails_fast() {
    let h = Harness::new();
    let Some(long_ago) = Instant::now().checked_sub(Duration::from_secs(120)) else {
        return;
    };
    *super::lock(&h.client.inner.link_down_since) = long_ago;
    let req = ResolveRequest {
        id: "r1".to_owned(),
        url: "https://www.youtube.com/watch?v=abc".to_owned(),
        ..ResolveRequest::default()
    };

    let started = std::time::Instant::now();
    let result = h
        .client
        .resolve(&req, Instant::now() + Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(super::lock(&h.client.inner.pending).is_empty());
    assert!(super::lock(&h.client.inner.inflight_cids).is_empty());
}

#[tokio::test]
async fn every_action_the_watchdog_receives_is_handled() {
    let contract: Value = serde_json::from_str(include_str!("../../tests/fixtures/mesh-contract.json"))
        .unwrap_or_else(|e| panic!("{e}"));
    let samples = contract["json_samples"]
        .as_object()
        .unwrap_or_else(|| panic!("no samples"));
    let mut expected: Vec<&str> = contract["watchdog_received_actions"]
        .as_array()
        .unwrap_or_else(|| panic!("no actions"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut names: Vec<&str> = samples.keys().map(String::as_str).collect();
    expected.sort_unstable();
    names.sort_unstable();
    assert_eq!(expected, names);

    let mut problems = Vec::new();
    for (name, sample) in samples {
        let h = Harness::v3(true);
        h.json(sample.as_str().unwrap_or_default()).await;
        let out = h.out.text();
        if out.contains("unknown action") || out.contains("parse failed") {
            problems.push(format!("{name}: {}", out.trim()));
        }
    }
    assert!(problems.is_empty(), "{problems:?}");
}

#[tokio::test]
async fn feedback_without_a_link_is_dropped() {
    let h = Harness::new();
    h.client.send_playback_feedback(crate::ipc::Feedback {
        url: "https://example.com/v".to_owned(),
        kind: vrcr_shared::wire::feedback::PLAYING,
        ms_since_open: 10,
        delivered_height: None,
        detail: None,
        correlation_id: None,
    });
    assert!(!h.client.is_connected());
    drop(h.dir);
}
