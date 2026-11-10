use super::*;

#[test]
fn describe_connect_failure_reports_the_root_cause() {
    let cause =
        ConnectError::Handshake("server returned HTTP 503 instead of 101".to_owned()).describe();
    assert_eq!(
        describe_connect_failure(
            "vrcresolver.com",
            15,
            &probe(Phase::Upgrade, Some(1), Some(2)),
            Some(&cause)
        ),
        "connect to vrcresolver.com failed after 15 ms during upgrade (dns 1 ms, tcp 2 ms): http_503"
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
        (
            json!({"correlation_id":"c1"}),
            "[mesh] playback quality cid=c1",
        ),
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
fn playback_feedback_payload_drops_empty_optionals() {
    let at = std::time::UNIX_EPOCH + Duration::from_secs(1_790_704_800);
    let text = playback_feedback_payload(vrcr_watchdog::mesh::frames::PlaybackFeedback {
        context: None,
        url: "https://x",
        kind: feedback::PLAYING,
        ms_since_open: 1200,
        client_id: "cid0",
        correlation_id: Some(""),
        timestamp: at,
        delivered_height: Some(0),
        detail: Some(""),
    });
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v["action"], action::PLAYBACK_FEEDBACK);
    assert_eq!(v["kind"], feedback::PLAYING);
    assert_eq!(v["ms_since_open"], 1200);
    assert_eq!(v["client_id"], "cid0");
    assert!(v.get("correlation_id").is_none_or(Value::is_null));
    assert!(v.get("delivered_height").is_none_or(Value::is_null));
    assert!(v.get("detail").is_none_or(Value::is_null));
    let text = playback_feedback_payload(vrcr_watchdog::mesh::frames::PlaybackFeedback {
        context: None,
        url: "https://x",
        kind: feedback::OG_FAILED,
        ms_since_open: 5,
        client_id: "c",
        correlation_id: Some("k1"),
        timestamp: at,
        delivered_height: Some(720),
        detail: Some("d"),
    });
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v["correlation_id"], "k1");
    assert_eq!(v["delivered_height"], 720);
    assert_eq!(v["detail"], "d");
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
    assert_eq!(
        path_and_query("https://a.b/watch?v=x&t=5"),
        "/watch?v=x&t=5"
    );
    assert_eq!(path_and_query("https://a.b/clip"), "/clip");
    assert_eq!(path_and_query("::"), "");
}

#[test]
fn sanitize_scrubs_identity_paths_ips_and_tokens() {
    let (post, _) = recording_post(200);
    let (r, _) = reporter(true, post);
    let url = "https://youtu.be/dQw4w9WgXcQ";
    let raw = format!(
        "C:\\Users\\alice\\x.txt on DESKTOP-1 at 10.0.0.5 key abcdefghijklmnopqrstuvwxyz for {url}"
    );
    assert_eq!(
        r.sanitize(&raw, Some(url)),
        "<path> on <machine> at <ip> key <token> for <url>"
    );
    assert_eq!(
        r.sanitize("/home/bob/.cache/x failed", None),
        "<path> failed"
    );
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
    assert_eq!(
        p.url_path_hash_short,
        hash_short("/watch?v=dQw4w9WgXcQ&t=5")
    );
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
