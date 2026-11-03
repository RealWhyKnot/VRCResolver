use super::*;

#[test]
fn connect_error_names_the_kind() {
    let io = || std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
    assert_eq!(
        ConnectError::Url("bad".to_owned()).describe(),
        "UrlError: bad"
    );
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
        assert_eq!(
            describe_connect_failure("vrcresolver.com", 8000, &p, cause),
            want
        );
    }
}

#[test]
fn describe_connect_failure_omits_timings_that_were_never_recorded() {
    assert_eq!(
        describe_connect_failure(
            "vrcresolver.com",
            8000,
            &probe(Phase::Dns, None, None),
            None
        ),
        "connect to vrcresolver.com timed out after 8000 ms during dns"
    );
}

#[test]
fn describe_connect_failure_strips_control_characters_from_the_cause() {
    let line = describe_connect_failure("h", 1, &probe(Phase::Dns, None, None), Some("a\r\nb"));
    assert!(!line.contains('\r') && !line.contains('\n'), "{line}");
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
    assert!(
        fallback_native_line("a", Some(fallback::UNITY_UNSUPPORTED_FORMAT)).contains("try AVPro")
    );
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
fn failure_kinds_follow_the_old_mapping() {
    assert_eq!(
        failure_kind(fallback::ALL_CONFIGS_FAILED),
        Some("AllStrategiesFailed")
    );
    assert_eq!(
        failure_kind(fallback::EXTRACTOR_UNSUPPORTED),
        Some("Unknown")
    );
    assert_eq!(failure_kind(fallback::INTERNAL_ERROR), Some("Unknown"));
    assert_eq!(
        failure_kind(fallback::UNITY_UNSUPPORTED_FORMAT),
        Some("Unknown")
    );
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
    assert!(
        out.text().contains("post failed: dns failure"),
        "{}",
        out.text()
    );
}
