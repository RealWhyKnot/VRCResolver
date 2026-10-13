mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use common::ManualClock;
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use vrcr_shared::console::Ux;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::iso8601_utc;
use vrcr_shared::wire::{Player, ResolveRequest, ResolveResponse, action, fallback, feedback, og_reason};
use vrcr_watchdog::health::{self, HealthGate, Transition};
use vrcr_watchdog::ipc::server::{
    FULL_WINDOW_BUDGET_MS, effective_budget_ms, is_healthy_outcome, og_failed_detail_for,
};
use vrcr_watchdog::ipc::{Feedback, Ipc, Mesh, MeshResult, framing};
use vrcr_watchdog::og_hint::OgFallbackHint;
use vrcr_watchdog::resolve_cache::{Key, ResolveCache};

const URL: &str = "https://example.com/v";
const NODE: &str = "us1.vrcresolver.com";
const CACHED_MEDIA: &str = "https://cache.example/v.mp4";
const MESH_MEDIA: &str = "https://media.example/v.mp4";

#[derive(Clone)]
enum Step {
    Reply {
        action: &'static str,
        reason: Option<&'static str>,
        url: Option<&'static str>,
        extra: Option<(&'static str, &'static str)>,
        retry_after_ms: Option<i32>,
    },
    Hang,
    Fail,
}

fn resolved_step() -> Step {
    Step::Reply {
        action: action::RESOLVED,
        reason: None,
        url: Some(MESH_MEDIA),
        extra: None,
        retry_after_ms: None,
    }
}

fn fallback_step(reason: &'static str) -> Step {
    Step::Reply {
        action: action::FALLBACK_NATIVE,
        reason: Some(reason),
        url: None,
        extra: None,
        retry_after_ms: None,
    }
}

#[derive(Default)]
struct FakeMesh {
    connected: bool,
    script: Mutex<VecDeque<Step>>,
    repeat: Mutex<Option<Step>>,
    resolves: Mutex<Vec<ResolveRequest>>,
    feedback: Mutex<Vec<Feedback>>,
}

impl FakeMesh {
    fn connected(steps: Vec<Step>) -> Arc<FakeMesh> {
        Arc::new(FakeMesh {
            connected: true,
            script: Mutex::new(steps.into()),
            ..FakeMesh::default()
        })
    }

    fn always(step: Step) -> Arc<FakeMesh> {
        let m = FakeMesh::connected(Vec::new());
        *m.repeat.lock().unwrap_or_else(|e| e.into_inner()) = Some(step);
        m
    }

    fn offline() -> Arc<FakeMesh> {
        Arc::new(FakeMesh::default())
    }

    fn resolves(&self) -> Vec<ResolveRequest> {
        self.resolves.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn feedback(&self) -> Vec<Feedback> {
        self.feedback.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Mesh for FakeMesh {
    fn current_node_host(&self) -> String {
        NODE.to_owned()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn resolve(&self, req: &ResolveRequest, _deadline: Instant) -> Result<MeshResult, String> {
        self.resolves
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.clone());
        let step = if !self.connected {
            fallback_step(fallback::SERVER_UNREACHABLE)
        } else {
            let next = self.script.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            next.or_else(|| self.repeat.lock().unwrap_or_else(|e| e.into_inner()).clone())
                .unwrap_or_else(|| fallback_step(fallback::SERVER_UNREACHABLE))
        };
        match step {
            Step::Hang => std::future::pending().await,
            Step::Fail => Err("boom".to_owned()),
            Step::Reply {
                action,
                reason,
                url,
                extra,
                retry_after_ms,
            } => {
                let mut resp = ResolveResponse {
                    action: action.to_owned(),
                    id: req.id.clone(),
                    reason: reason.map(str::to_owned),
                    url: url.map(str::to_owned),
                    retry_after_ms,
                    ..ResolveResponse::default()
                };
                if let Some((k, v)) = extra {
                    resp.extra.insert(k.to_owned(), Value::String(v.to_owned()));
                }
                Ok(MeshResult {
                    frame: serde_json::to_vec(&resp).unwrap_or_default(),
                    action: action.to_owned(),
                    reason: reason.map(str::to_owned),
                    retry_after_ms,
                })
            }
        }
    }

    fn send_playback_feedback(&self, f: Feedback) {
        self.feedback.lock().unwrap_or_else(|e| e.into_inner()).push(f);
    }
}

async fn exchange(ipc: &Ipc<FakeMesh>, line: &str) -> String {
    let (mut client, mut server) = tokio::io::duplex(1 << 20);
    client
        .write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    ipc.handle(&mut server).await;
    drop(server);
    let mut out = String::new();
    client
        .read_to_string(&mut out)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    out
}

async fn ask(ipc: &Ipc<FakeMesh>, line: &str) -> Value {
    let out = exchange(ipc, line).await;
    let mut lines = out.lines();
    let first = lines.next().unwrap_or_else(|| panic!("no reply for {line}"));
    assert!(lines.next().is_none(), "more than one reply: {out}");
    serde_json::from_str(first).unwrap_or_else(|e| panic!("{e}: {first}"))
}

fn req(player: &str, extra: &str) -> String {
    format!("{{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"{URL}\",\"player\":\"{player}\"{extra}}}")
}

fn open_gate(now: vrcr_watchdog::clock::Now) -> Arc<HealthGate> {
    let g = HealthGate::new(health::DEFAULT_COOLDOWN, health::DEFAULT_VERDICT_TIMEOUT, now);
    for _ in 0..health::OPEN_THRESHOLD {
        g.record_resolve_outcome(false, false);
    }
    Arc::new(g)
}

fn seeded_cache(dir: &TempDir) -> Arc<ResolveCache> {
    let clock = ManualClock::at(SystemTime::now());
    let cache = ResolveCache::new(
        dir.path().join("resolve_cache.json"),
        clock.now(),
        Log::disabled(),
    );
    cache.store(
        &Key {
            node: NODE,
            url: URL,
            player: Some("unity"),
            format_arg: None,
            max_height: Some(720),
        },
        &ResolveResponse {
            action: action::RESOLVED.to_owned(),
            url: Some(CACHED_MEDIA.to_owned()),
            expires_at: Some(iso8601_utc(SystemTime::now() + Duration::from_secs(3600))),
            ..ResolveResponse::default()
        },
    );
    Arc::new(cache)
}

const UNITY_720: &str = "{\"action\":\"resolve\",\"id\":\"probe1\",\"url\":\"https://example.com/v\",\"player\":\"unity\",\"maxHeight\":720}";

fn reason(v: &Value) -> &str {
    v["reason"].as_str().unwrap_or_default()
}

#[test]
fn is_healthy_outcome_counts_server_and_internal_failures_as_unhealthy() {
    type Case<'a> = (Option<&'a str>, &'a str, Option<&'a str>, i32, bool);
    let cases: [Case; 15] = [
        (None, "resolved", None, 26_500, true),
        (None, "fallback_native", Some("all_configs_failed"), 26_500, true),
        (None, "fallback_native", Some("rate_limited"), 26_500, true),
        (None, "fallback_native", Some("protocol_error"), 26_500, true),
        (None, "fallback_native", Some("warp_down"), 26_500, true),
        (None, "fallback_native", Some("server_unreachable"), 26_500, false),
        (None, "fallback_native", Some("internal_error"), 26_500, false),
        (
            Some("server_unreachable"),
            "fallback_native/server_unreachable",
            None,
            26_500,
            false,
        ),
        (None, "fallback_native", Some("server_unreachable"), 3_000, false),
        (
            None,
            "fallback_native",
            Some("client_deadline_exceeded"),
            26_500,
            false,
        ),
        (
            None,
            "fallback_native",
            Some("client_deadline_exceeded"),
            3_000,
            true,
        ),
        (
            Some("client_deadline_exceeded"),
            "fallback_native/client_deadline_exceeded",
            None,
            26_500,
            false,
        ),
        (
            Some("client_deadline_exceeded"),
            "fallback_native/client_deadline_exceeded",
            None,
            3_000,
            true,
        ),
        (
            None,
            "fallback_native",
            Some("client_deadline_exceeded"),
            20_000,
            false,
        ),
        (
            None,
            "fallback_native",
            Some("client_deadline_exceeded"),
            19_999,
            true,
        ),
    ];
    for (fail, outcome, server, budget, want) in cases {
        assert_eq!(
            is_healthy_outcome(fail, outcome, server, budget),
            want,
            "{fail:?} {outcome} {server:?} {budget}"
        );
    }
    assert_eq!(FULL_WINDOW_BUDGET_MS, 20_000);
}

#[test]
fn og_failed_detail_collapses_to_the_server_set() {
    for (input, want) in [
        (Some("cf_403"), "cf_403"),
        (Some("rate_limited"), "rate_limited"),
        (Some("sign_in_required"), "sign_in_required"),
        (Some("content_not_found"), "content_not_found"),
        (Some("unknown"), "unknown"),
        (Some("something_else"), "unknown"),
        (None, "unknown"),
    ] {
        assert_eq!(og_failed_detail_for(input), want);
    }
}

#[test]
fn effective_budget_keeps_a_margin_and_clamps() {
    assert_eq!(effective_budget_ms(26_500), 26_000);
    assert_eq!(effective_budget_ms(1_200), 1_000);
    assert_eq!(effective_budget_ms(-5), 1_000);
    assert_eq!(effective_budget_ms(500_000), 90_000);
    assert_eq!(effective_budget_ms(i32::MIN), 1_000);
}

#[test]
fn player_labels_match_the_old_console() {
    let mut r = ResolveRequest {
        player: Some(Player::Unity),
        max_height: Some(720),
        ..ResolveRequest::default()
    };
    assert_eq!(framing::player_label(&r), "Unity 720p");
    r.player = Some(Player::AvPro);
    r.max_height = None;
    r.vrchat_format_arg = Some("(mp4/best)[height<=?1080][height>=?64]".to_owned());
    assert_eq!(framing::player_label(&r), "AVPro max");
    r.vrchat_format_arg = Some("best[height<=1440]".to_owned());
    assert_eq!(framing::player_label(&r), "AVPro 1440p");
    r.vrchat_format_arg = None;
    r.player = None;
    assert_eq!(framing::player_label(&r), "AVPro max");
}

#[test]
fn wrapper_event_detection_only_looks_at_the_head() {
    assert!(framing::looks_like_wrapper_event(
        "{\"action\":\"og_fallback_notify\"}"
    ));
    assert!(framing::looks_like_wrapper_event(
        "{\"action\":\"wrapper_og_failed\"}"
    ));
    let late = format!(
        "{{\"pad\":\"{}\",\"action\":\"wrapper_og_failed\"}}",
        "x".repeat(300)
    );
    assert!(!framing::looks_like_wrapper_event(&late));
    assert!(framing::is_localhost_youtube_url(
        "http://LOCALHOST.youtube.com:1234/x"
    ));
    assert!(!framing::is_localhost_youtube_url(
        "https://www.youtube.com/watch?v=a"
    ));
}

#[tokio::test]
async fn malformed_json_yields_internal_error_fallback() {
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled()).build();
    let v = ask(&ipc, "{not json at all").await;
    assert_eq!(v["action"], action::FALLBACK_NATIVE);
    assert_eq!(reason(&v), fallback::INTERNAL_ERROR);
    assert_eq!(v["id"], "");
}

#[tokio::test]
async fn empty_null_and_urlless_requests_are_rejected() {
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled()).build();
    for line in [
        "",
        "null",
        "{\"action\":\"resolve\",\"id\":\"a\",\"player\":\"avpro\"}",
    ] {
        assert_eq!(
            reason(&ask(&ipc, line).await),
            fallback::INTERNAL_ERROR,
            "{line:?}"
        );
    }
}

#[tokio::test]
async fn non_resolve_action_is_rejected_with_its_id() {
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled()).build();
    let v = ask(
        &ipc,
        "{\"action\":\"ping\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"avpro\"}",
    )
    .await;
    assert_eq!(reason(&v), fallback::INTERNAL_ERROR);
    assert_eq!(v["id"], "x1");
}

#[tokio::test]
async fn player_vocabulary_is_case_sensitive() {
    let mesh = FakeMesh::offline();
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled()).build();
    for player in ["AVPro", "Unity", ""] {
        assert_eq!(
            reason(&ask(&ipc, &req(player, "")).await),
            fallback::INTERNAL_ERROR,
            "{player}"
        );
    }
    let missing = "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\"}";
    assert_eq!(reason(&ask(&ipc, missing).await), fallback::INTERNAL_ERROR);
    assert!(mesh.resolves().is_empty());
}

#[tokio::test]
async fn oversized_request_without_newline_is_rejected() {
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled()).build();
    let (mut client, mut server) = tokio::io::duplex(1 << 16);
    let writer = tokio::spawn(async move {
        let chunk = vec![b'a'; 1 << 16];
        let mut sent = 0;
        while sent <= framing::MAX_REQUEST_BYTES {
            if client.write_all(&chunk).await.is_err() {
                break;
            }
            sent += chunk.len();
        }
        let mut out = Vec::new();
        let _ = client.read_to_end(&mut out).await;
        out
    });
    ipc.handle(&mut server).await;
    drop(server);
    let out = writer.await.unwrap_or_default();
    let v: Value = serde_json::from_slice(out.strip_suffix(b"\n").unwrap_or(&out)).unwrap_or_default();
    assert_eq!(reason(&v), fallback::INTERNAL_ERROR);
}

#[tokio::test]
async fn og_fallback_hint_short_circuits_to_prior_load_failure() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(URL);
    let mesh = FakeMesh::offline();
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .og_hint(hint)
        .build();
    assert_eq!(
        reason(&ask(&ipc, &req("avpro", "")).await),
        og_reason::PRIOR_LOAD_FAILURE
    );
    assert!(mesh.resolves().is_empty());
}

#[tokio::test]
async fn skip_native_hint_clears_the_hint_and_asks_the_mesh() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(URL);
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .og_hint(hint.clone())
        .build();
    let v = ask(&ipc, &req("avpro", ",\"skip_native_hint\":true")).await;
    assert_eq!(v["url"], MESH_MEDIA);
    assert_eq!(hint.live_entry_count(), 0);
}

#[tokio::test]
async fn open_health_gate_short_circuits_to_resolver_unhealthy() {
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .health(open_gate(vrcr_watchdog::clock::system()))
        .build();
    assert_eq!(
        reason(&ask(&ipc, &req("avpro", "")).await),
        og_reason::RESOLVER_UNHEALTHY
    );
    assert!(mesh.resolves().is_empty());
}

#[tokio::test(start_paused = true)]
async fn open_health_gate_lets_the_wrapper_re_ask_through() {
    let mesh = FakeMesh::offline();
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .health(open_gate(vrcr_watchdog::clock::system()))
        .build();
    let v = ask(&ipc, &req("avpro", ",\"skip_native_hint\":true")).await;
    assert_eq!(reason(&v), fallback::SERVER_UNREACHABLE);
    assert_eq!(mesh.resolves().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn open_health_gate_keeps_sites_with_blocked_og_on_the_server() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_og_blocked("https://youtu.be/abc");
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled())
        .og_hint(hint)
        .health(open_gate(vrcr_watchdog::clock::system()))
        .build();
    let line = "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://www.youtube.com/watch?v=def\",\"player\":\"avpro\"}";
    assert_eq!(reason(&ask(&ipc, line).await), fallback::SERVER_UNREACHABLE);
}

#[tokio::test(start_paused = true)]
async fn prior_load_failure_does_not_send_a_blocked_site_to_og() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure("https://youtu.be/abc");
    hint.record_og_blocked("https://youtu.be/abc");
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled())
        .og_hint(hint)
        .build();
    let line = "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://youtu.be/abc\",\"player\":\"avpro\"}";
    assert_eq!(reason(&ask(&ipc, line).await), fallback::SERVER_UNREACHABLE);
}

#[tokio::test]
async fn og_failed_notify_blocks_og_only_for_site_wide_reasons() {
    for (why, blocked) in [
        ("sign_in_required", true),
        ("cf_403", true),
        ("rate_limited", true),
        ("content_not_found", false),
        ("timeout", false),
    ] {
        let hint = Arc::new(OgFallbackHint::default());
        hint.record_load_failure("https://youtu.be/abc");
        let mesh = FakeMesh::offline();
        let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
            .og_hint(hint.clone())
            .build();
        let line = format!(
            "{{\"action\":\"wrapper_og_failed\",\"url\":\"https://youtu.be/abc\",\"reason\":\"{why}\",\"exit_code\":1,\"rid\":\"r1\"}}"
        );
        assert_eq!(exchange(&ipc, &line).await, "", "{why}");
        assert_eq!(hint.live_entry_count(), 0, "{why}");
        assert_eq!(
            hint.is_og_blocked("https://www.youtube.com/watch?v=zzz"),
            blocked,
            "{why}"
        );
        let fb = mesh.feedback();
        assert_eq!(fb.len(), 1);
        assert_eq!(fb[0].kind, feedback::OG_FAILED);
        assert_eq!(fb[0].detail.as_deref(), Some(og_failed_detail_for(Some(why))));
        assert_eq!(fb[0].correlation_id.as_deref(), Some("r1"));
    }
}

#[tokio::test]
async fn og_failed_notify_evicts_the_cached_url() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = seeded_cache(&dir);
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled())
        .cache(cache.clone())
        .build();
    let line = format!(
        "{{\"action\":\"wrapper_og_failed\",\"url\":\"{CACHED_MEDIA}\",\"reason\":\"cf_403\",\"exit_code\":1}}"
    );
    exchange(&ipc, &line).await;
    assert_eq!(cache.count(), 0);
}

#[tokio::test]
async fn og_fallback_notify_reports_rejected_urls_only() {
    for (why, reported) in [
        (og_reason::RESOLVED_URL_REJECTED, true),
        (og_reason::AVPRO_INCOMPATIBLE, true),
        (og_reason::PIPE_CONNECT_FAILED, false),
    ] {
        let mesh = FakeMesh::offline();
        let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled()).build();
        let line = format!(
            "{{\"action\":\"og_fallback_notify\",\"url\":\"{URL}\",\"reason\":\"{why}\",\"elapsed_ms\":12,\"rid\":\"r9\",\"public_message\":\"hi\"}}"
        );
        assert_eq!(exchange(&ipc, &line).await, "");
        let fb = mesh.feedback();
        assert_eq!(fb.len(), usize::from(reported), "{why}");
        if reported {
            assert_eq!(fb[0].kind, feedback::RESOLVED_REJECTED);
            assert_eq!(fb[0].detail.as_deref(), Some(why));
        }
    }
}

#[tokio::test]
async fn probe_request_skips_the_cache() {
    let clock = ManualClock::at(SystemTime::now());
    let gate = open_gate(clock.now());
    clock.advance(health::DEFAULT_COOLDOWN + Duration::from_secs(1));
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .cache(seeded_cache(&dir))
        .health(gate.clone())
        .build();
    let v = ask(&ipc, UNITY_720).await;
    assert_eq!(v["action"], action::FALLBACK_NATIVE);
    assert_eq!(mesh.resolves().len(), 1);
    assert!(!gate.awaiting_probe_verdict());
    assert!(!gate.should_short_circuit(true).0);
}

#[tokio::test]
async fn resolved_probe_comes_from_the_mesh_and_waits_for_playback() {
    let clock = ManualClock::at(SystemTime::now());
    let gate = open_gate(clock.now());
    clock.advance(health::DEFAULT_COOLDOWN + Duration::from_secs(1));
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .cache(seeded_cache(&dir))
        .health(gate.clone())
        .build();
    let v = ask(&ipc, UNITY_720).await;
    assert_eq!(v["url"], MESH_MEDIA);
    assert_eq!(mesh.resolves().len(), 1);
    assert!(gate.awaiting_probe_verdict());
    assert_eq!(gate.record_playback_confirmed(), Transition::Closed);
}

#[tokio::test]
async fn closed_gate_serves_from_the_cache_and_reports_the_cache_play() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .cache(seeded_cache(&dir))
        .health(Arc::new(HealthGate::default()))
        .build();
    let v = ask(&ipc, UNITY_720).await;
    assert_eq!(v["url"], CACHED_MEDIA);
    assert_eq!(v["id"], "probe1");
    assert!(mesh.resolves().is_empty());
    let fb = mesh.feedback();
    assert_eq!(fb.len(), 1);
    assert_eq!(fb[0].kind, feedback::CACHE_PLAY);
    assert_eq!(fb[0].correlation_id.as_deref(), Some("probe1"));
    assert_eq!(ipc.stats().activity().resolves_cache_hits, 1);
}

#[tokio::test]
async fn resolved_mesh_answers_are_cached_for_the_next_ask() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = Arc::new(ResolveCache::new(
        dir.path().join("resolve_cache.json"),
        vrcr_watchdog::clock::system(),
        Log::disabled(),
    ));
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .cache(cache.clone())
        .build();
    assert_eq!(ask(&ipc, UNITY_720).await["url"], MESH_MEDIA);
    assert_eq!(cache.count(), 1);
    let again = ask(&ipc, &UNITY_720.replace("probe1", "second")).await;
    assert_eq!(again["url"], MESH_MEDIA);
    assert_eq!(again["id"], "second");
    assert_eq!(mesh.resolves().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn deadline_outcomes_open_the_gate_only_for_a_full_window() {
    for (wrapper_ms, server_answers, expect_open) in
        [(1_500, false, false), (26_500, true, true), (1_500, true, false)]
    {
        let gate = Arc::new(HealthGate::default());
        let step = if server_answers {
            Step::Reply {
                action: action::FALLBACK_NATIVE,
                reason: Some(fallback::CLIENT_DEADLINE_EXCEEDED),
                url: None,
                extra: Some(("public_message", "scripted")),
                retry_after_ms: None,
            }
        } else {
            Step::Hang
        };
        let mesh = FakeMesh::always(step);
        let ipc = Ipc::builder(mesh, Ux::sink(), Log::disabled())
            .health(gate.clone())
            .build();
        for i in 0..health::OPEN_THRESHOLD {
            let line = format!(
                "{{\"action\":\"resolve\",\"id\":\"deadline{i}\",\"url\":\"{URL}\",\"player\":\"unity\",\"maxHeight\":720,\"wrapper_deadline_ms\":{wrapper_ms}}}"
            );
            let v = ask(&ipc, &line).await;
            assert_eq!(reason(&v), fallback::CLIENT_DEADLINE_EXCEEDED);
            assert_eq!(v.get("public_message").is_some(), server_answers);
        }
        assert_eq!(
            gate.should_short_circuit(true).0,
            expect_open,
            "{wrapper_ms} {server_answers}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn hung_mesh_hits_the_default_window_without_opening_the_gate() {
    let gate = Arc::new(HealthGate::default());
    let ipc = Ipc::builder(FakeMesh::always(Step::Hang), Ux::sink(), Log::disabled())
        .health(gate.clone())
        .build();
    let started = Instant::now();
    let v = ask(&ipc, UNITY_720).await;
    assert_eq!(reason(&v), fallback::CLIENT_DEADLINE_EXCEEDED);
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    for _ in 1..health::OPEN_THRESHOLD {
        ask(&ipc, UNITY_720).await;
    }
    assert!(!gate.should_short_circuit(true).0);
}

#[tokio::test]
async fn mesh_errors_become_internal_error_fallbacks_and_get_reported() {
    let reported = Arc::new(Mutex::new(Vec::new()));
    let sink = reported.clone();
    let ipc = Ipc::builder(FakeMesh::connected(vec![Step::Fail]), Ux::sink(), Log::disabled())
        .report_fallback(move |_, r| sink.lock().unwrap_or_else(|e| e.into_inner()).push(r.to_owned()))
        .build();
    assert_eq!(reason(&ask(&ipc, UNITY_720).await), fallback::INTERNAL_ERROR);
    assert_eq!(
        *reported.lock().unwrap_or_else(|e| e.into_inner()),
        [fallback::INTERNAL_ERROR]
    );
}

#[tokio::test(start_paused = true)]
async fn discovery_in_progress_is_retried_with_a_fresh_id() {
    let mesh = FakeMesh::connected(vec![
        Step::Reply {
            action: action::FALLBACK_NATIVE,
            reason: Some(fallback::DISCOVERY_IN_PROGRESS),
            url: None,
            extra: None,
            retry_after_ms: Some(400),
        },
        resolved_step(),
    ]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled()).build();
    let started = Instant::now();
    let v = ask(&ipc, UNITY_720).await;
    assert_eq!(v["url"], MESH_MEDIA);
    assert_eq!(started.elapsed(), Duration::from_millis(400));
    let sent = mesh.resolves();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].id, "probe1");
    assert_ne!(sent[1].id, "probe1");
    assert_eq!(sent[1].id.len(), 32);
}

#[tokio::test(start_paused = true)]
async fn non_retryable_fallbacks_are_forwarded_at_once() {
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled()).build();
    assert_eq!(reason(&ask(&ipc, UNITY_720).await), fallback::ALL_CONFIGS_FAILED);
    assert_eq!(mesh.resolves().len(), 1);
}

#[tokio::test]
async fn requests_are_shaped_before_they_reach_the_mesh() {
    let mesh = FakeMesh::always(resolved_step());
    let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .verified_codecs(|| Some(vec!["vp9".to_owned()]))
        .build();
    ask(
        &ipc,
        &req("avpro", ",\"accept_codecs\":[\"h264\"],\"maxHeight\":2160"),
    )
    .await;
    ask(&ipc, &req("avpro", ",\"maxHeight\":2160")).await;
    let hq = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled())
        .high_quality(|| true)
        .build();
    ask(&hq, &req("avpro", ",\"maxHeight\":720")).await;
    ask(&hq, &req("unity", ",\"maxHeight\":720")).await;

    let sent = mesh.resolves();
    let codecs = sent[0].accept_codecs.clone().unwrap_or_default();
    assert!(
        codecs.contains(&"vp9".to_owned()) && !codecs.contains(&"av1".to_owned()),
        "{codecs:?}"
    );
    assert_eq!(sent[0].correlation_id.as_deref(), Some("x1"));
    assert_eq!(sent[0].max_height, Some(1080));
    assert_eq!(sent[1].correlation_id, None);
    assert_eq!(sent[2].max_height, Some(2160));
    assert_eq!(sent[2].prefer_highest, Some(true));
    assert_eq!(sent[3].max_height, Some(720));
}

#[cfg(windows)]
mod pipe {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::windows::named_pipe::ClientOptions;
    use vrcr_watchdog::ipc::pipe::{current_user_sid, pipe_sddl};

    #[test]
    fn sddl_grants_the_user_and_system_and_labels_the_pipe_low() {
        let sid = current_user_sid().unwrap_or_else(|e| panic!("{e}"));
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(
            pipe_sddl("S-1-5-21-1"),
            "O:S-1-5-21-1G:S-1-5-21-1D:(A;;0x1f019f;;;S-1-5-21-1)(A;;0x1f019f;;;SY)S:(ML;;NW;;;LW)"
        );
    }

    async fn connect(name: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
        let path = format!(r"\\.\pipe\{name}");
        for _ in 0..200 {
            if let Ok(c) = ClientOptions::new().open(&path) {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("pipe never came up");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_named_pipe_serves_concurrent_clients() {
        let name = format!("vrcresolver.test.{}", uuid::Uuid::new_v4().simple());
        let mesh = FakeMesh::always(resolved_step());
        let ipc = Ipc::builder(mesh.clone(), Ux::sink(), Log::disabled()).build();
        let server = ipc.serve(&name);

        let mut tasks = Vec::new();
        for i in 0..4 {
            let name = name.clone();
            tasks.push(tokio::spawn(async move {
                let client = connect(&name).await;
                let mut reader = BufReader::new(client);
                let line = if i == 0 {
                    "{not json".to_owned()
                } else {
                    format!(
                        "{{\"action\":\"resolve\",\"id\":\"p{i}\",\"url\":\"{URL}\",\"player\":\"avpro\"}}"
                    )
                };
                reader
                    .get_mut()
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .unwrap_or_else(|e| panic!("{e}"));
                let mut reply = String::new();
                reader
                    .read_line(&mut reply)
                    .await
                    .unwrap_or_else(|e| panic!("{e}"));
                let mut rest = String::new();
                let _ = reader.read_line(&mut rest).await;
                (i, reply, rest)
            }));
        }
        for t in tasks {
            let (i, reply, rest) = t.await.unwrap_or_else(|e| panic!("{e}"));
            let v: Value = serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"));
            if i == 0 {
                assert_eq!(reason(&v), fallback::INTERNAL_ERROR);
            } else {
                assert_eq!(v["url"], MESH_MEDIA);
                assert_eq!(v["id"], format!("p{i}"));
            }
            assert_eq!(rest, "", "server should hang up after one reply");
        }
        assert_eq!(mesh.resolves().len(), 3);
        server.abort();
    }
}
