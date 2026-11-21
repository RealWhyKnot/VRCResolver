use super::*;

#[tokio::test]
async fn og_result_after_a_server_fallback_is_final() {
    let rig = Rig::new();
    let mut server = StubPipe::start(&rig.pipe_name, Some(PRIOR_FAILURE_REPLY), Duration::ZERO);
    install_fake_og(&rig);
    let code = rig
        .shim()
        .run(&strings(&[
            "/c",
            "echo og-stdout& echo ERROR: Unsupported URL 1>&2& exit 1",
            URL,
        ]))
        .await;
    assert_eq!(code, 1);
    assert!(rig.stdout.text().contains("og-stdout"));
    let lines = server.lines_until_quiet(QUIET).await;
    assert_eq!(
        actions(&lines),
        ["resolve", "og_fallback_notify", "wrapper_og_failed"]
    );
    assert_eq!(lines[1]["reason"], og_reason::SERVER_FALLBACK_NATIVE);
    let log = rig.log();
    assert!(
        log.contains("FALLBACK og result is final reason=server_fallback_native"),
        "{log}"
    );
    assert!(
        log.contains("FALLBACK og stderr: ERROR: Unsupported URL"),
        "{log}"
    );
    assert!(!log.contains("re-ask"), "{log}");
}

#[tokio::test]
async fn og_success_after_a_server_fallback_is_passed_through() {
    let rig = Rig::new();
    let _server = StubPipe::start(&rig.pipe_name, Some(PRIOR_FAILURE_REPLY), Duration::ZERO);
    install_fake_og(&rig);
    let code = rig
        .shim()
        .run(&strings(&[
            "/c",
            "echo https://og.example.com/v.mp4& exit 0",
            URL,
        ]))
        .await;
    assert_eq!(code, 0);
    assert!(
        rig.stdout.text().contains("https://og.example.com/v.mp4"),
        "{}",
        rig.stdout.text()
    );
}

#[tokio::test]
async fn a_local_pipe_failure_still_lets_the_wrapper_re_ask_after_og_fails() {
    let rig = Rig::new();
    let mut server = StubPipe::start(
        &rig.pipe_name,
        Some(r#"{"action":"mystery","id":"srv1"}"#),
        Duration::ZERO,
    );
    install_fake_og(&rig);
    let code = rig.shim().run(&strings(&["/c", "exit 1", URL])).await;
    assert_eq!(code, 1);
    let lines = server.lines_until_quiet(QUIET).await;
    let resolves: Vec<_> = lines.iter().filter(|l| l["action"] == "resolve").collect();
    assert_eq!(resolves.len(), 2, "{lines:?}");
    assert_eq!(resolves[1]["skip_native_hint"], true);
    assert!(!rig.log().contains("og result is final"));
}

#[tokio::test]
async fn cdn4_live_playlist_with_a_prior_failure_falls_back_to_the_raw_url() {
    let rig = Rig::new();
    let mut server = StubPipe::start(&rig.pipe_name, Some(PRIOR_FAILURE_REPLY), Duration::ZERO);
    install_fake_og(&rig);
    let code = rig
        .shim()
        .run(&strings(&["--no-check-certificate", CDN4_LIVE]))
        .await;
    assert_eq!(code, 0);
    assert_eq!(rig.stdout.text(), format!("{CDN4_LIVE}\n"));
    let lines = server.lines_until_quiet(QUIET).await;
    assert_eq!(actions(&lines), ["resolve", "og_fallback_notify"]);
    assert_eq!(lines[1]["url"], CDN4_LIVE);
    let log = rig.log();
    assert!(log.contains("outcome=direct-media-fallback"), "{log}");
    assert!(log.contains("host=live.cdn4.io"), "{log}");
    assert!(!log.contains("FALLBACK og: spawning"), "{log}");
}

#[tokio::test]
async fn vrcdn_live_stream_with_a_prior_failure_falls_back_to_the_raw_url() {
    let rig = Rig::new();
    let mut server = StubPipe::start(&rig.pipe_name, Some(PRIOR_FAILURE_REPLY), Duration::ZERO);
    let code = rig.shim().run(&strings(&[VRCDN_LIVE])).await;
    assert_eq!(code, 0);
    assert_eq!(rig.stdout.text(), format!("{VRCDN_LIVE}\n"));
    let lines = server.lines_until_quiet(QUIET).await;
    assert_eq!(actions(&lines), ["resolve", "og_fallback_notify"]);
}

#[tokio::test]
async fn direct_media_that_is_unsafe_to_emit_still_goes_through_og() {
    let rig = Rig::new();
    let _server = StubPipe::start(&rig.pipe_name, Some(PRIOR_FAILURE_REPLY), Duration::ZERO);
    install_fake_og(&rig);
    let code = rig
        .shim()
        .run(&strings(&["/c", "exit 1", "http://192.168.1.10/live.m3u8"]))
        .await;
    assert_eq!(code, 1);
    assert_eq!(rig.stdout.text(), "");
    assert!(rig.log().contains("FALLBACK og: spawning"));
}

fn listening_relay(rig: &Rig) -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a relay port");
    let port = listener.local_addr().expect("relay port").port();
    let state = rig.dir.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    std::fs::write(state.join("relay_port.txt"), port.to_string()).expect("write relay port");
    (listener, port)
}

fn assert_relay_wrapped(out: &str, port: u16, direct: &str) {
    assert!(
        out.starts_with(&format!("http://localhost.youtube.com:{port}/play/")),
        "{out}"
    );
    assert_eq!(
        vrcr_shared::trust_gateway::extract_target(out.trim()).as_deref(),
        Some(direct)
    );
}

#[tokio::test]
async fn first_party_proxy_urls_stay_on_our_relay_when_the_watchdog_falls_back() {
    for direct in [
        "https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
        "https://vrcresolver.com/api/proxy?q=abc",
    ] {
        let rig = Rig::new();
        let (_listener, port) = listening_relay(&rig);
        install_fake_og(&rig);
        let mut server = StubPipe::start(&rig.pipe_name, Some(PRIOR_FAILURE_REPLY), Duration::ZERO);
        let code = rig.shim().run(&[direct.to_owned()]).await;
        assert_eq!(code, 0, "{direct}");
        assert_relay_wrapped(&rig.stdout.text(), port, direct);
        let lines = server.lines_until_quiet(QUIET).await;
        assert_eq!(actions(&lines), ["resolve"], "{direct}");
        let log = rig.log();
        assert!(!log.contains("FALLBACK og"), "{log}");
        assert!(
            log.contains("outcome=direct-first-party-playback-wrapped"),
            "{log}"
        );
    }
}

#[tokio::test]
async fn first_party_proxy_urls_are_wrapped_at_once_when_the_watchdog_is_down() {
    for direct in [
        "https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
        "https://vrcresolver.com/api/proxy?q=abc",
    ] {
        let rig = Rig::new();
        let (_listener, port) = listening_relay(&rig);
        install_fake_og(&rig);
        let started = Instant::now();
        let code = rig.shim().run(&[direct.to_owned()]).await;
        let elapsed = started.elapsed();
        assert_eq!(code, 0, "{direct}");
        assert_relay_wrapped(&rig.stdout.text(), port, direct);
        let log = rig.log();
        assert!(!log.contains("FALLBACK og"), "{log}");
        assert!(!log.contains("pipe connect retry"), "{log}");
        assert!(log.contains("pipe connect ENOENT"), "{log}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }
}

#[tokio::test]
async fn first_party_proxy_urls_play_what_the_watchdog_returns() {
    let rig = Rig::new();
    let line = resolved_line(RESOLVED);
    let _server = StubPipe::start(&rig.pipe_name, Some(&line), Duration::ZERO);
    let direct = "https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc";
    let code = rig.shim().run(&[direct.to_owned()]).await;
    assert_eq!(code, 0);
    assert_eq!(rig.stdout.text(), format!("{RESOLVED}\n"));
    assert!(rig.log().contains("outcome=direct-first-party-playback "));
}
