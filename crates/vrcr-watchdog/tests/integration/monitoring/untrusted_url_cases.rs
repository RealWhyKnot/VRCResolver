use super::*;

const UNTRUSTED_NOTICE: &str = "Enable Allow Untrusted URLs in VRChat Settings > Comfort & Safety.";
const SPOTIFY_PLAYER_ERROR: &str =
    "[ACO_Bgm] error PlayerError https://open.spotify.com/track/test\n";
const SPOTIFY_TRACK: &str = "https://open.spotify.com/track/test";
const UNTRUSTED_BLOCK: &str =
    "[Video Playback] Blocked untrusted URL: https://example.test/video.mp4\n";

#[test]
fn untrusted_url_notice_is_once_per_log_session() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut monitor = r.monitor();
    let first = logs.write("output_log_a.txt", "    Allow untrusted URLs: True\n");
    monitor.poll_once(r.now());
    logs.append(&first, SPOTIFY_PLAYER_ERROR);
    logs.append(&first, UNTRUSTED_BLOCK);
    monitor.poll_once(r.now());
    assert!(!r.out.text().contains(UNTRUSTED_NOTICE));

    r.clock.advance(secs(10));
    let second = logs.write("output_log_b.txt", UNTRUSTED_BLOCK);
    monitor.poll_once(r.now());
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);

    r.clock.advance(secs(10));
    logs.append(&first, SPOTIFY_PLAYER_ERROR);
    monitor.poll_once(r.now());
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);

    logs.append(&first, "    Allow untrusted URLs: False\n");
    logs.append(&first, UNTRUSTED_BLOCK);
    logs.append(&first, UNTRUSTED_BLOCK);
    monitor.poll_once(r.now());
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 2);

    r.clock.advance(secs(10));
    logs.append(&second, UNTRUSTED_BLOCK);
    monitor.poll_once(r.now());
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 2);
}

#[test]
fn spotify_player_error_reaching_ipc_reports_failure_and_explicit_denial_prompts() {
    let r = Rig::new();
    let stats = Arc::new(Stats::new(r.now()));
    stats
        .playback
        .record_resolve_request("https://open.spotify.com/track/test?si=example", r.now());
    let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
    deps.stats = Some(stats);
    let mut monitor = r.monitor_with(deps);
    r.feed(&mut monitor, SPOTIFY_PLAYER_ERROR);
    assert!(!r.out.text().contains(UNTRUSTED_NOTICE));
    assert_eq!(r.sink.kinds(), vec![feedback::LOAD_FAILURE]);
    r.feed(
        &mut monitor,
        &format!("[Video Playback] Blocked untrusted URL: {SPOTIFY_TRACK}\n"),
    );
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);
    assert_eq!(r.sink.kinds(), vec![feedback::LOAD_FAILURE]);
}

#[test]
fn spotify_player_error_without_ipc_prompts_without_blaming_open_video() {
    for url in [
        SPOTIFY_TRACK,
        "https://PLAY.SPOTIFY.COM/track/test",
        "https://spotify.link/test",
    ] {
        let r = Rig::new();
        let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
        deps.stats = Some(Arc::new(Stats::new(r.now())));
        let mut monitor = r.monitor_with(deps);
        r.feed(&mut monitor, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
        let error = format!("[ProTV] Video Error: PlayerError url='{url}'\n");
        r.feed(&mut monitor, &error);
        r.feed(&mut monitor, &error);
        assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);
        assert_eq!(r.sink.kinds().len(), 0);
        assert!(monitor.stall_armed());
    }
}

#[test]
fn spotify_url_after_the_failing_video_preserves_failure_and_cleanup() {
    let r = Rig::new().with_cache();
    let mut monitor = r.monitor();
    let playback = format!("{PLAYBACK}(part),extra");
    r.store(&playback);
    r.feed(&mut monitor, &format!("{}\n", opening(&playback)));
    assert!(monitor.stall_armed());

    r.feed(
        &mut monitor,
        &format!("[ACO_Bgm] error PlayerError '{playback}' (previous: {SPOTIFY_TRACK})\n"),
    );

    assert_eq!(
        r.sink.all(),
        vec![(playback.clone(), feedback::LOAD_FAILURE, 0, None)]
    );
    assert!(r.out.text().contains("VRChat couldn't load the video."));
    assert!(!r.out.text().contains(UNTRUSTED_NOTICE));
    assert_eq!(monitor.last_opened_url(), None);
    assert_eq!(monitor.next_deadline(), None);
    assert!(!r.cached(&playback));
}

#[test]
fn spotify_url_after_a_failing_ipv6_video_preserves_failure_and_cleanup() {
    const IPV6_VIDEO: &str = "https://[2001:db8::1]/video.mp4";
    let r = Rig::new().with_cache();
    let mut monitor = r.monitor();
    r.store(IPV6_VIDEO);
    assert!(r.cached(IPV6_VIDEO));
    r.feed(&mut monitor, &format!("{}\n", opening(IPV6_VIDEO)));
    assert!(monitor.stall_armed());

    r.feed(
        &mut monitor,
        &format!("[ACO_Bgm] error PlayerError url='{IPV6_VIDEO}' (previous: {SPOTIFY_TRACK})\n"),
    );

    assert_eq!(
        r.sink.all(),
        vec![(IPV6_VIDEO.to_owned(), feedback::LOAD_FAILURE, 0, None)]
    );
    assert!(r.out.text().contains("VRChat couldn't load the video."));
    assert!(!r.out.text().contains(UNTRUSTED_NOTICE));
    assert_eq!(monitor.last_opened_url(), None);
    assert_eq!(monitor.next_deadline(), None);
    assert!(!r.cached(IPV6_VIDEO));
}

#[test]
fn spotify_url_after_a_video_url_preserves_failure_and_cleanup() {
    for (playback, logged_url) in [
        (
            "https://[2001:db8::1]/video(1).mp4",
            "url='https://[2001:db8::1]/video(1).mp4'",
        ),
        (
            "https://example.com/a(1).mp4",
            r#"url="https://example.com/a(1).mp4","#,
        ),
        (
            "https://example.com/a(1).mp4",
            "(https://example.com/a(1).mp4).",
        ),
        ("https://example.com/a(1)", "https://example.com/a(1);"),
        (
            "https://example.com/qa's-video.mp4",
            "https://example.com/qa's-video.mp4",
        ),
        (
            "https://example.com/qa's-video.mp4",
            r#"https://example.com/qa's-video.mp4'",;."#,
        ),
    ] {
        let r = Rig::new().with_cache();
        let mut monitor = r.monitor();
        r.store(playback);
        assert!(r.cached(playback));
        r.feed(&mut monitor, &format!("{}\n", opening(playback)));
        assert!(monitor.stall_armed());

        r.feed(
            &mut monitor,
            &format!("[ACO_Bgm] error PlayerError {logged_url} (previous: {SPOTIFY_TRACK})\n"),
        );

        assert_eq!(
            r.sink.all(),
            vec![(playback.to_owned(), feedback::LOAD_FAILURE, 0, None)],
            "{logged_url}"
        );
        assert!(r.out.text().contains("VRChat couldn't load the video."));
        assert!(!r.out.text().contains(UNTRUSTED_NOTICE));
        assert_eq!(monitor.last_opened_url(), None);
        assert_eq!(monitor.next_deadline(), None);
        assert!(!r.cached(playback));
    }
}

#[test]
fn ordinary_video_errors_and_spotify_lookalike_hosts_do_not_prompt() {
    let r = Rig::new();
    let mut monitor = r.monitor();
    r.feed(
        &mut monitor,
        &format!(
            "[Video Playback] Allow untrusted URLs: False\n\
             [Unrelated] Blocking an untrusted URL\n\
             [ACO_Bgm] error PlayerError https://open.spotify.com.example.test/track/test\n\
             [VideoTXL] Video error event: PlayerError RateLimited {SPOTIFY_TRACK}\n"
        ),
    );
    assert!(!r.out.text().contains(UNTRUSTED_NOTICE));
    r.feed(
        &mut monitor,
        "[VIDEO PLAYER] Not allowed UNTRUSTED URL https://example.test/video.mp4\n",
    );
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);
}

#[test]
fn spotify_player_error_uses_the_latest_logged_untrusted_url_setting() {
    let r = Rig::new();
    let mut monitor = r.monitor();
    r.feed(
        &mut monitor,
        "2026.11.24 19:04:06 Log        -      Allow untrusted URLs: True\n",
    );
    r.feed(&mut monitor, SPOTIFY_PLAYER_ERROR);
    assert!(!r.out.text().contains(UNTRUSTED_NOTICE));

    r.feed(&mut monitor, "Allow untrusted URLs: False\n");
    r.feed(&mut monitor, SPOTIFY_PLAYER_ERROR);
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);
}

#[test]
fn access_denied_requires_video_context_and_a_setting_other_than_true() {
    for error in [
        "[Video Playback] Error: AccessDenied\n",
        "[Video Player] VideoError.AccessDenied\n",
        "[ACO_Bgm] PlayerError (AccessDenied)\n",
    ] {
        let r = Rig::new();
        let mut monitor = r.monitor();
        r.feed(
            &mut monitor,
            "[Image Download] AccessDenied\n[Video Playback] OtherAccessDenied\n",
        );
        assert!(!r.out.text().contains(UNTRUSTED_NOTICE));
        r.feed(&mut monitor, "Allow untrusted URLs: True\n");
        r.feed(&mut monitor, error);
        assert!(!r.out.text().contains(UNTRUSTED_NOTICE), "{error}");
        r.feed(&mut monitor, "Allow untrusted URLs: False\n");
        r.feed(&mut monitor, error);
        r.feed(&mut monitor, error);
        assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1, "{error}");
    }

    let r = Rig::new();
    let mut monitor = r.monitor();
    r.feed(&mut monitor, "[Video Playback] Error: AccessDenied\n");
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);
}

#[test]
fn same_path_log_truncation_resets_the_notice_and_setting() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut monitor = r.monitor();
    let log = logs.write("output_log_a.txt", "boot\n");
    monitor.poll_once(r.now());
    logs.append(&log, UNTRUSTED_BLOCK);
    logs.append(&log, "Allow untrusted URLs: True\n");
    monitor.poll_once(r.now());
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);

    logs.write("output_log_a.txt", SPOTIFY_PLAYER_ERROR);
    monitor.poll_once(r.now());
    monitor.poll_once(r.now());
    assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 2);
}

#[test]
fn avpro_untrusted_url_failures_report_and_clear_playback_before_and_after_notice() {
    let r = Rig::new().with_cache();
    let mut monitor = r.monitor();
    let failure = "[AVProVideo] Error: Loading failed. This is an untrusted URL";
    for (index, error) in [format!("{failure}\n"), format!("{failure}: {PLAYBACK}\n")]
        .iter()
        .enumerate()
    {
        r.store(PLAYBACK);
        r.feed(&mut monitor, &format!("{}\n", opening(PLAYBACK)));
        assert!(monitor.stall_armed());
        r.feed(&mut monitor, error);
        assert_eq!(r.out.text().matches(UNTRUSTED_NOTICE).count(), 1);
        assert_eq!(
            r.out
                .text()
                .matches("VRChat couldn't load the video.")
                .count(),
            index + 1
        );
        assert_eq!(r.sink.kinds(), vec![feedback::LOAD_FAILURE; index + 1]);
        assert_eq!(monitor.last_opened_url(), None);
        assert_eq!(monitor.next_deadline(), None);
        assert!(!r.cached(PLAYBACK));
        r.clock.advance(secs(11));
    }
}
