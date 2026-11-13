use super::*;

#[test]
fn is_healthy_outcome_counts_server_and_internal_failures_as_unhealthy() {
    type Case<'a> = (Option<&'a str>, &'a str, Option<&'a str>, i32, bool);
    let cases: [Case; 15] = [
        (None, "resolved", None, 26_500, true),
        (
            None,
            "fallback_native",
            Some("all_configs_failed"),
            26_500,
            true,
        ),
        (None, "fallback_native", Some("rate_limited"), 26_500, true),
        (
            None,
            "fallback_native",
            Some("protocol_error"),
            26_500,
            true,
        ),
        (None, "fallback_native", Some("warp_down"), 26_500, true),
        (
            None,
            "fallback_native",
            Some("server_unreachable"),
            26_500,
            false,
        ),
        (
            None,
            "fallback_native",
            Some("internal_error"),
            26_500,
            false,
        ),
        (
            Some("server_unreachable"),
            "fallback_native/server_unreachable",
            None,
            26_500,
            false,
        ),
        (
            None,
            "fallback_native",
            Some("server_unreachable"),
            3_000,
            false,
        ),
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

#[tokio::test]
async fn open_health_gate_short_circuits_to_resolver_unhealthy() {
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .health(open_gate(vrcr_watchdog::clock::system()))
        .build();
    assert_eq!(
        reason(&ask(&ipc, &req("avpro", "")).await),
        og_reason::RESOLVER_UNHEALTHY
    );
    assert_eq!(
        mesh.resolves(),
        <[vrcr_shared::wire::ResolveRequest; 0]>::default()
    );
}

#[tokio::test(start_paused = true)]
async fn open_health_gate_lets_the_wrapper_re_ask_through() {
    let mesh = FakeMesh::offline();
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
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

#[tokio::test]
async fn probe_request_skips_the_cache() {
    let clock = ManualClock::at(SystemTime::now());
    let gate = open_gate(clock.now());
    clock.advance(health::DEFAULT_COOLDOWN + Duration::from_secs(1));
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(seeded_cache(&dir))
        .health(std::sync::Arc::clone(&gate))
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
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(seeded_cache(&dir))
        .health(std::sync::Arc::clone(&gate))
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
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(seeded_cache(&dir))
        .health(Arc::new(HealthGate::default()))
        .build();
    let v = ask(&ipc, UNITY_720).await;
    assert_eq!(v["url"], CACHED_MEDIA);
    assert_eq!(v["id"], "probe1");
    assert_eq!(
        mesh.resolves(),
        <[vrcr_shared::wire::ResolveRequest; 0]>::default()
    );
    let fb = mesh.feedback();
    assert_eq!(fb.len(), 1);
    assert_eq!(fb[0].kind, feedback::CACHE_PLAY);
    assert_eq!(fb[0].correlation_id.as_deref(), Some("probe1"));
    assert_eq!(ipc.stats().activity().resolves_cache_hits, 1);
}

#[tokio::test(start_paused = true)]
async fn deadline_outcomes_open_the_gate_only_for_a_full_window() {
    for (wrapper_ms, server_answers, expect_open) in [
        (1_500, false, false),
        (26_500, true, true),
        (1_500, true, false),
    ] {
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
        let output = crate::common::mesh::Capture::default();
        let ux = output.ux();
        let ipc = Ipc::builder(mesh, ux, Log::disabled())
            .health(std::sync::Arc::clone(&gate))
            .build();
        for i in 0..health::OPEN_THRESHOLD {
            let line = format!(
                "{{\"action\":\"resolve\",\"id\":\"deadline{i}\",\"url\":\"{URL}\",\"player\":\"unity\",\"maxHeight\":720,\"wrapper_deadline_ms\":{wrapper_ms}}}"
            );
            let v = ask(&ipc, &line).await;
            assert_eq!(reason(&v), fallback::CLIENT_DEADLINE_EXCEEDED);
            assert!(v.get("public_message").is_none());
            assert_eq!(
                output
                    .text()
                    .matches("scripted VRChat will try it on its own.")
                    .count(),
                if server_answers {
                    usize::try_from(i + 1).expect("small resolve count")
                } else {
                    0
                }
            );
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
        .health(std::sync::Arc::clone(&gate))
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
