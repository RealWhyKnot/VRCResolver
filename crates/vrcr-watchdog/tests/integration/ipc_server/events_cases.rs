use super::*;

#[tokio::test]
async fn wrapper_and_player_failure_print_only_the_first_row_and_keep_both_logs() {
    use std::sync::RwLock;
    use vrcr_shared::first_party::FirstPartyPolicy;
    use vrcr_watchdog::monitoring::vrclog::{MonitorDeps, VrcLogMonitor};
    use vrcr_watchdog::stats::Stats;

    for wrapper_first in [true, false] {
        let dir = TempDir::new().expect("test directory");
        let out = crate::common::mesh::Capture::default();
        let ux = out.ux();
        let mesh = FakeMesh::always(fallback_step(fallback::ALL_CONFIGS_FAILED));
        let stats = Arc::new(Stats::new(SystemTime::now()));
        let ipc = Ipc::builder(Arc::clone(&mesh), ux.clone(), Log::disabled())
            .stats(Arc::clone(&stats))
            .build();
        let mut deps = MonitorDeps::new(
            mesh,
            Arc::new(RwLock::new(FirstPartyPolicy::default())),
            ux,
            Log::disabled(),
        );
        deps.stats = Some(stats);
        let mut monitor = VrcLogMonitor::new(deps, dir.path().to_owned());
        let notification = serde_json::json!({"action": action::WRAPPER_OG_FAILED, "url": URL, "reason": "timeout", "rid": "wrapper-run"}).to_string();
        for expected in 1..=2 {
            let reply = ask(&ipc, &req("avpro", "")).await;
            assert_eq!(reply["action"], action::FALLBACK_NATIVE);
            monitor.process_text(
                &format!("[AVProVideo] Opening {URL} (offset 0) with API MediaFoundation\n"),
                SystemTime::now(),
            );
            if wrapper_first {
                exchange(&ipc, &notification).await;
            }
            monitor.process_text("[AVProVideo] Error: Loading failed\n", SystemTime::now());
            exchange(&ipc, &notification).await;
            let text = out.text();
            assert_eq!(text.matches("Failed").count(), expected, "{text}");
            let first = if wrapper_first {
                "VRChat couldn't play it either."
            } else {
                "VRChat couldn't load the video."
            };
            let second = if wrapper_first {
                "VRChat couldn't load the video."
            } else {
                "VRChat couldn't play it either."
            };
            assert_eq!(text.matches(first).count(), expected);
            assert!(!text.contains(second));
            let log = out.logs();
            assert_eq!(log.matches("[vrclog] load_failure ms=").count(), expected);
            assert_eq!(
                log.matches("[wrapper][warn] !! og also failed").count(),
                expected * if wrapper_first { 2 } else { 1 }
            );
        }
    }
}

#[tokio::test]
async fn fallback_message_is_printed_once_across_resolve_and_wrapper_notification() {
    let message = "The video is private.";
    let mesh = FakeMesh::always(Step::Reply {
        action: action::FALLBACK_NATIVE,
        reason: Some(fallback::ALL_CONFIGS_FAILED),
        url: None,
        extra: Some(("public_message", message)),
        retry_after_ms: Some(123),
    });
    let output = crate::common::mesh::Capture::default();
    let ux = output.ux();
    let ipc = Ipc::builder(mesh, ux, Log::disabled()).build();
    for expected in 1..=2 {
        let response = ask(&ipc, &req("avpro", "")).await;
        assert_eq!(response["action"], action::FALLBACK_NATIVE);
        assert_eq!(response["reason"], fallback::ALL_CONFIGS_FAILED);
        assert_eq!(response["retry_after_ms"], 123);
        assert!(response.get("public_message").is_none());
        let notification = serde_json::json!({
            "action": action::OG_FALLBACK_NOTIFY,
            "url": URL,
            "reason": og_reason::SERVER_FALLBACK_NATIVE,
            "rid": "wrapper-run",
            "public_message": response.get("public_message"),
        });
        exchange(&ipc, &notification.to_string()).await;
        let text = output.text();
        assert_eq!(text.matches(message).count(), expected);
        assert_eq!(text.matches("Skipped").count(), expected);
        assert_eq!(text.matches("[wrapper] !!").count(), 0);
        assert_eq!(text.lines().count(), expected);
        assert_eq!(output.logs().matches("[wrapper] !!").count(), expected);
        assert!(!text.contains("[wrapper][warn]"));
    }
    let standalone = serde_json::json!({
        "action": action::OG_FALLBACK_NOTIFY,
        "url": URL,
        "reason": og_reason::SERVER_FALLBACK_NATIVE,
        "rid": "another-wrapper",
        "public_message": message,
    });
    exchange(&ipc, &standalone.to_string()).await;
    assert_eq!(output.text().matches(message).count(), 3);
    assert!(
        output
            .logs()
            .contains("[wrapper][warn] The video is private.")
    );
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
        let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
            .og_hint(std::sync::Arc::clone(&hint))
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
        assert_eq!(
            fb[0].detail.as_deref(),
            Some(og_failed_detail_for(Some(why)))
        );
        assert_eq!(fb[0].correlation_id.as_deref(), Some("r1"));
    }
}

#[tokio::test]
async fn legacy_failure_previews_are_not_logged_and_feedback_is_preserved() {
    let directory = TempDir::new().expect("temporary log directory");
    let path = directory.path().join("ipc.log");
    let log = Log::to_file(&path);
    let ux = Ux::sink();
    ux.tee_to(log.clone());
    let mesh = FakeMesh::offline();
    let ipc = Ipc::builder(Arc::clone(&mesh), ux, log.clone()).build();
    let payload = serde_json::json!({
        "action": action::WRAPPER_OG_FAILED,
        "url": CACHED_MEDIA,
        "reason": "cf_403",
        "exit_code": 1,
        "elapsed_ms": 123,
        "rid": "legacy-failure",
        "error_preview": "cookie=private-cookie password=private-password bearer private-token"
    });
    assert_eq!(exchange(&ipc, &payload.to_string()).await, "");
    let feedback = mesh.feedback();
    let [event] = feedback.as_slice() else {
        panic!("one playback failure is reported");
    };
    assert_eq!(event.kind, feedback::OG_FAILED);
    assert_eq!(event.url, CACHED_MEDIA);
    assert_eq!(event.detail.as_deref(), Some("cf_403"));
    assert_eq!(event.correlation_id.as_deref(), Some("legacy-failure"));
    log.close();
    let text = std::fs::read_to_string(path).expect("IPC log");
    assert!(text.contains("wrapper_og_failed"));
    assert!(!text.contains("private-"), "{text}");
    assert!(!text.contains("preview="), "{text}");
}

#[tokio::test]
async fn og_failed_notify_evicts_the_cached_url() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = seeded_cache(&dir);
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled())
        .cache(std::sync::Arc::clone(&cache))
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
        let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled()).build();
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
async fn bundled_failure_is_one_plain_row_with_technical_detail_in_the_log() {
    let output = crate::common::mesh::Capture::default();
    let ipc = Ipc::builder(FakeMesh::offline(), output.ux(), Log::disabled()).build();
    exchange(&ipc, r#"{"action":"wrapper_og_failed","url":"https://youtube.com/watch?v=secret","reason":"content_not_found","exit_code":1}"#).await;
    assert_eq!(
        output.text(),
        format!(
            "  Failed     {:<28}VRChat couldn't play it either.\n",
            "youtube.com"
        )
    );
    assert!(output.logs().contains("reason=content_not_found exit=1"));
    assert!(!output.text().contains("secret"));
}
