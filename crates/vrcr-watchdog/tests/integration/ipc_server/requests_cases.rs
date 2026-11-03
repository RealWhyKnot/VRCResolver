use super::*;

#[test]
fn effective_budget_keeps_a_margin_and_clamps() {
    assert_eq!(effective_budget_ms(26_500), 26_000);
    assert_eq!(effective_budget_ms(1_200), 1_000);
    assert_eq!(effective_budget_ms(-5), 1_000);
    assert_eq!(effective_budget_ms(500_000), 90_000);
    assert_eq!(effective_budget_ms(i32::MIN), 1_000);
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
async fn malformed_requests_do_not_log_payloads_or_invalid_values() {
    let directory = TempDir::new().expect("temporary log directory");
    let path = directory.path().join("ipc.log");
    let log = Log::to_file(&path);
    let ux = Ux::sink();
    ux.tee_to(log.clone());
    let ipc = Ipc::builder(FakeMesh::offline(), ux, log.clone()).build();
    for payload in [
        r#"{"password":"private-password","#,
        r#"{"action":"resolve","protocol_version":"private-version"}"#,
        r#"{"action":"wrapper_og_failed","exit_code":"private-exit"}"#,
    ] {
        exchange(&ipc, payload).await;
    }
    log.close();
    let text = std::fs::read_to_string(path).expect("IPC log");
    assert!(text.contains("parse failed"));
    assert!(!text.contains("private-"), "{text}");
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
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled()).build();
    for player in ["AVPro", "Unity", ""] {
        assert_eq!(
            reason(&ask(&ipc, &req(player, "")).await),
            fallback::INTERNAL_ERROR,
            "{player}"
        );
    }
    let missing = "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\"}";
    assert_eq!(reason(&ask(&ipc, missing).await), fallback::INTERNAL_ERROR);
    assert_eq!(
        mesh.resolves(),
        <[vrcr_shared::wire::ResolveRequest; 0]>::default()
    );
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
    let v: Value =
        serde_json::from_slice(out.strip_suffix(b"\n").unwrap_or(&out)).unwrap_or_default();
    assert_eq!(reason(&v), fallback::INTERNAL_ERROR);
}

#[tokio::test]
async fn og_fallback_hint_short_circuits_to_prior_load_failure() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(URL);
    let mesh = FakeMesh::offline();
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .og_hint(hint)
        .build();
    assert_eq!(
        reason(&ask(&ipc, &req("avpro", "")).await),
        og_reason::PRIOR_LOAD_FAILURE
    );
    assert_eq!(
        mesh.resolves(),
        <[vrcr_shared::wire::ResolveRequest; 0]>::default()
    );
}

#[tokio::test]
async fn mesh_errors_become_internal_error_fallbacks_and_get_reported() {
    let reported = Arc::new(Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&reported);
    let ipc = Ipc::builder(
        FakeMesh::connected(vec![Step::Fail]),
        Ux::sink(),
        Log::disabled(),
    )
    .report_fallback(move |_, r| {
        sink.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(r.to_owned())
    })
    .build();
    assert_eq!(
        reason(&ask(&ipc, UNITY_720).await),
        fallback::INTERNAL_ERROR
    );
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
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled()).build();
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
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled()).build();
    assert_eq!(
        reason(&ask(&ipc, UNITY_720).await),
        fallback::ALL_CONFIGS_FAILED
    );
    assert_eq!(mesh.resolves().len(), 1);
}

#[tokio::test]
async fn requests_are_shaped_before_they_reach_the_mesh() {
    let mesh = FakeMesh::always(resolved_step());
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .verified_codecs(|| Some(vec!["vp9".to_owned()]))
        .build();
    ask(
        &ipc,
        &req("avpro", ",\"accept_codecs\":[\"h264\"],\"maxHeight\":2160"),
    )
    .await;
    ask(&ipc, &req("avpro", ",\"maxHeight\":2160")).await;
    let hq = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
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
