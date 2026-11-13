use super::*;

struct ReceivedFallback(MeshResult);

impl Mesh for ReceivedFallback {
    fn current_node_host(&self) -> String {
        "us1.vrcresolver.com".to_owned()
    }
    fn is_connected(&self) -> bool {
        true
    }
    async fn resolve(&self, _: &ResolveRequest, _: Instant) -> Result<MeshResult, String> {
        Ok(self.0.clone())
    }
    fn send_playback_feedback(&self, _: crate::ipc::Feedback) {}
}

#[tokio::test]
async fn fallback_public_message_reaches_one_ipc_outcome_and_resolve_logs_stay_in_file() {
    use tokio::io::AsyncWriteExt;
    for binary in [false, true] {
        let harness = Harness::new();
        let mut pending = harness.pending("r1");
        let message = "\x1b[31mThe\x1b[0m\t video\n is private.\u{2603}";
        if binary {
            harness
                .binary(&mp_frame(vec![
                    mp_str(action::FALLBACK_NATIVE),
                    mp_str("r1"),
                    mp_str(fallback::ALL_CONFIGS_FAILED),
                    mp_str(message),
                ]))
                .await;
        } else {
            harness
                .json(
                    &serde_json::json!({
                        "action": action::FALLBACK_NATIVE, "id": "r1",
                        "reason": fallback::ALL_CONFIGS_FAILED, "public_message": message,
                    })
                    .to_string(),
                )
                .await;
        }
        harness
            .json(r#"{"action":"resolve_log","id":"r1","message":"trying extractor_only"}"#)
            .await;
        let result = done(&mut pending).expect("fallback");
        let ipc = crate::ipc::Ipc::builder(
            Arc::new(ReceivedFallback(result)),
            harness.client.inner.ux.clone(),
            Log::to_file(&harness.log_path),
        )
        .build();
        let (mut client, mut server) = tokio::io::duplex(65536);
        client.write_all(b"{\"action\":\"resolve\",\"id\":\"r1\",\"url\":\"https://youtube.com/watch?v=secret\",\"player\":\"avpro\"}\n").await.expect("request");
        ipc.handle(&mut server).await;
        let text = harness.out.text();
        assert_eq!(text.matches("Skipped").count(), 1, "{text}");
        assert!(
            text.contains("The video is private. VRChat will try it on its own."),
            "{text}"
        );
        assert!(text.is_ascii());
        assert!(!text.contains("extractor_only"));
        assert!(!text.contains("[resolve_log]"));
        assert!(!text.contains("secret"));
        assert!(
            harness
                .log()
                .contains("[resolve_log] id=r1 level=info phase=trying config=extractor_only")
        );
    }
}

#[tokio::test]
async fn rate_limited_non_resolve_action_sets_no_cooldown() {
    let h = Harness::new();

    h.json(r#"{"action":"rate_limited","meshAction":"playback_feedback","limit":30,"windowSeconds":60,"retryAfterSeconds":2}"#)
        .await;

    assert!(h.cooldown().is_none());
    assert!(
        h.log()
            .contains("server rate-limited playback_feedback -- pausing for 2s")
    );
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
        h.log()
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
    assert_eq!(
        result.reason.as_deref(),
        Some(fallback::DISCOVERY_IN_PROGRESS)
    );
    assert_eq!(result.retry_after_ms, Some(800));
    assert!(
        h.log()
            .contains("[mesh] fallback_native id=r1 reason=discovery_in_progress")
    );
    assert_eq!(h.out.text(), "");
}

#[tokio::test]
async fn unknown_action_is_discarded_with_a_warning() {
    let h = Harness::new();

    h.json(r#"{"action":"frobnicate"}"#).await;

    assert!(h.log().contains("unknown action -- discarding: frobnicate"));
}

#[tokio::test]
async fn repeated_parse_failures_warn_once_a_minute() {
    let h = Harness::new();

    h.json("{not json").await;
    h.json("{still not json").await;

    let text = h.log();
    assert_eq!(text.matches("frame parse failed").count(), 1, "{text}");
    assert!(text.contains("frame parse failed (json x1 in last min)"));
}

#[tokio::test]
async fn malformed_frames_do_not_log_payloads() {
    let h = Harness::new();
    let secret = "private-stream-key";
    h.json(&format!(
        r#"{{"url":"https://example.com/path?token={secret}","#
    ))
    .await;
    let mut binary = vec![0xc1];
    binary.extend_from_slice(secret.as_bytes());
    h.binary(&binary).await;
    let output = format!("{}{}", h.out.text(), h.log());
    assert!(output.contains("frame parse failed"));
    assert!(!output.contains(secret), "{output}");
    assert!(!output.contains("example.com/path"), "{output}");
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
async fn text_pong_advances_the_last_pong() {
    let h = Harness::new();
    assert!(super::super::lock(&h.client.inner.last_pong).is_none());

    h.json(r#"{"action":"pong"}"#).await;

    assert!(super::super::lock(&h.client.inner.last_pong).is_some());
}

#[tokio::test]
async fn binary_pong_advances_the_last_pong() {
    let h = Harness::new();
    let before = Instant::now();

    h.binary(&mp_frame(vec![mp_str(action::PONG)])).await;

    assert!(super::super::lock(&h.client.inner.last_pong).is_some_and(|t| t >= before));
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
        h.log()
            .contains("unknown binary action -- discarding: frobnicate_widget")
    );
}

#[tokio::test]
async fn binary_control_frames_that_belong_on_text_are_flagged() {
    let h = Harness::new();

    h.binary(&mp_frame(vec![mp_str(action::RATE_LIMITED)]))
        .await;

    assert!(h.log().contains("rate_limited received via binary path"));
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
    let frame: ResolveResponse =
        serde_json::from_slice(&result.frame).unwrap_or_else(|e| panic!("{e}"));
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
    assert!(
        h.log()
            .contains("reason=warp_down (server WARP egress unhealthy")
    );
}

#[tokio::test]
async fn broken_binary_frames_warn_once_per_kind_a_minute() {
    let h = Harness::new();

    h.binary(&[0xC1]).await;
    h.binary(&[0xC1]).await;
    h.binary(&mp_frame(vec![])).await;

    let text = h.log();
    assert_eq!(
        text.matches("binary frame parse failed (binary:action peek")
            .count(),
        1,
        "{text}"
    );
    assert!(text.contains("empty msgpack array (count=0)"));
}

#[tokio::test]
async fn every_action_the_watchdog_receives_is_handled() {
    let contract: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mesh-contract.json"
    )))
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
        let out = h.log();
        if out.contains("unknown action") || out.contains("parse failed") {
            problems.push(format!("{name}: {}", out.trim()));
        }
    }
    assert!(problems.is_empty(), "{problems:?}");
}

#[tokio::test(start_paused = true)]
async fn one_outage_with_ten_failed_attempts_prints_only_loss_and_recovery() {
    let h = Harness::new();
    h.client.inner.console_connected();
    assert_eq!(h.out.text(), "  Connected to the server.\n");
    h.out.0.lock().expect("capture lock").clear();
    h.client.inner.report_failure("disconnected (error): test");
    for attempt in 1..=10 {
        h.client
            .inner
            .reconnect_attempt
            .store(attempt, std::sync::atomic::Ordering::SeqCst);
        h.client
            .inner
            .report_failure(&format!("attempt_{attempt}_failed"));
    }
    tokio::time::advance(Duration::from_secs(147)).await;
    h.client.inner.console_connected();
    assert_eq!(
        h.out.text(),
        "  Lost the connection to the server. Trying again...\n  Connected again after 2m 27s.\n"
    );
    let log = h.log();
    for attempt in 1..=10 {
        assert_eq!(
            log.matches(&format!(
                "reconnect attempt {attempt} failed: attempt_{attempt}_failed"
            ))
            .count(),
            1
        );
    }
    h.client.inner.shutdown.send_replace(true);
    h.client.inner.console_disconnected();
    assert_eq!(h.out.text().lines().count(), 2);
}
