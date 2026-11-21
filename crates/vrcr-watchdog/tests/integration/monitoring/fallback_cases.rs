use super::*;
use vrcr_watchdog::emitted::{EmittedUrls, Origin};

const OTHER: &str = "https://other.example.com/clip.mp4";

fn failures(r: &Rig) -> Vec<Feedback> {
    r.sink
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|f| f.kind == feedback::LOAD_FAILURE)
        .cloned()
        .collect()
}

fn with_emitted(r: &Rig) -> (VrcLogMonitor, Arc<EmittedUrls>) {
    let emitted = Arc::new(EmittedUrls::new(r.clock.now()));
    emitted.remember(
        PLAYBACK,
        &Origin {
            source_url: SOURCE.to_owned(),
            correlation_id: Some("req-9".to_owned()),
        },
    );
    let mut deps = r.deps(AudioCheck::new(AudioState::Ok, Some("Speakers")));
    deps.emitted = Some(Arc::clone(&emitted));
    (r.monitor_with(deps), emitted)
}

fn hint(r: &Rig) -> &OgFallbackHint {
    r.hint.as_deref().unwrap_or_else(|| panic!("hint"))
}

#[test]
fn a_load_failure_within_thirty_five_seconds_belongs_to_the_open() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(ms(34_000));
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    assert_eq!(
        r.sink.all(),
        vec![(PLAYBACK.to_owned(), feedback::LOAD_FAILURE, 34_000, None)]
    );
}

#[test]
fn a_load_failure_with_nothing_open_is_logged_not_dropped() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    assert_eq!(r.sink.all().len(), 0);
    assert!(
        r.file_log()
            .contains("[vrclog] load_failure with no open video error=")
    );
}

#[test]
fn a_mid_stream_error_after_playback_started_still_counts() {
    let r = Rig::new().with_cache();
    let (mut m, _) = with_emitted(&r);
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.feed(&mut m, "[AVProVideo] Using playback path: MF\n");
    assert!(!m.stall_armed());
    r.clock.advance(secs(90));
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    let failed = failures(&r);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].ms_since_open, 90_000);
    assert!(hint(&r).should_prefer_og(SOURCE));
}

#[test]
fn world_player_errors_count_as_load_failures() {
    for (line, detail) in [
        (
            "2026.11.20 21:04:11 Error      -  [ProTV] TVManager (Main TV)] Video Error: PlayerError",
            "protv_player_error",
        ),
        (
            "2026.11.20 21:04:11 Log        -  nowplaying {\"type\":\"mediaError\",\"errorMsg\":\"Livestream has stopped.\"}",
            "protv_media_error",
        ),
        (
            "2026.11.20 21:04:11 Log        -  [VideoTXL:AVPro-2] Video error event: PlayerError",
            "videotxl_player_error",
        ),
        (
            "2026.11.20 21:04:11 Log        -  [VideoTXL:SyncPlayer] Error code: Player Error",
            "videotxl_player_error",
        ),
        (
            "2026.11.20 21:04:11 Log        -  [USharpVideo] Video failed: https://example.com/x",
            "usharpvideo_error",
        ),
    ] {
        let r = Rig::new();
        let mut m = r.monitor();
        r.feed(
            &mut m,
            &format!("[AVProVideo] Opening {PLAYBACK}\n{line}\n"),
        );
        let failed = failures(&r);
        assert_eq!(failed.len(), 1, "{line}");
        assert_eq!(failed[0].detail.as_deref(), Some(detail), "{line}");
    }
}

#[test]
fn one_failure_reported_by_avpro_and_the_world_is_counted_once() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(
        &mut m,
        &format!(
            "[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Error: Loading failed\n[VideoTXL:AVPro-2] Video error event: PlayerError\n"
        ),
    );
    assert_eq!(failures(&r).len(), 1);
}

#[test]
fn rate_limit_errors_are_not_load_failures() {
    for line in [
        "[ProTV] TVManager (Main TV)] Video Error: RateLimited",
        "[VideoTXL:SyncPlayer] Error code: Player Error (Rate limited)",
        "[VideoTXL:AVPro-2] Video error event: PlayerError RateLimited",
    ] {
        let r = Rig::new();
        let mut m = r.monitor();
        r.feed(
            &mut m,
            &format!("[AVProVideo] Opening {PLAYBACK}\n{line}\n"),
        );
        assert_eq!(failures(&r).len(), 0, "{line}");
        assert!(m.stall_armed(), "{line}");
    }
}

#[test]
fn an_unrelated_load_url_keeps_the_stall_watchdog_armed() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(
        &mut m,
        &format!(
            "[AVProVideo] Opening {PLAYBACK}\n[VideoTXL:AVPro-2] Load Url: {OTHER}\nNow Playing: {OTHER}\n"
        ),
    );
    assert!(m.stall_armed());
    r.feed(
        &mut m,
        &format!("[VideoTXL:AVPro-2] Load Url: {PLAYBACK}\n"),
    );
    assert!(!m.stall_armed());
}

#[test]
fn a_load_url_for_the_source_link_clears_the_stall() {
    let r = Rig::new();
    let (mut m, _) = with_emitted(&r);
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {PLAYBACK}\nNow Playing: {SOURCE}\n"),
    );
    assert!(!m.stall_armed());
}

#[test]
fn emitted_urls_attribute_failures_to_their_source_and_request() {
    let r = Rig::new().with_cache();
    let (mut m, _) = with_emitted(&r);
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    assert!(hint(&r).should_prefer_og(SOURCE));
    assert!(hint(&r).should_prefer_og("https://virtualfilm.institute/watch?v=abc&utm_source=x"));
    let failed = failures(&r);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].correlation_id.as_deref(), Some("req-9"));
    assert_eq!(failed[0].source_url.as_deref(), Some(SOURCE));
}

#[test]
fn confirmed_playback_through_our_route_clears_the_failure_hint() {
    let r = Rig::new().with_cache();
    let (mut m, _) = with_emitted(&r);
    hint(&r).record_load_failure(SOURCE);
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(secs(2));
    r.feed(&mut m, "fwidth=1280 fheight=720\n");
    assert!(!hint(&r).should_prefer_og(SOURCE));
    assert!(r.file_log().contains("failure hint cleared"));
}

#[test]
fn starting_playback_alone_does_not_clear_the_failure_hint() {
    let r = Rig::new().with_cache();
    let (mut m, _) = with_emitted(&r);
    hint(&r).record_load_failure(SOURCE);
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Using playback path: MF\n"),
    );
    assert!(hint(&r).should_prefer_og(SOURCE));
}

#[test]
fn another_screens_world_error_does_not_blame_the_playing_video() {
    let r = Rig::new().with_cache();
    let (mut m, _) = with_emitted(&r);
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.feed(&mut m, "[AVProVideo] Using playback path: MF\n");
    r.clock.advance(secs(2));
    r.feed(&mut m, "fwidth=1280 fheight=720\n");
    r.clock.advance(secs(90));
    r.feed(
        &mut m,
        "2026.11.20 21:04:11 Error      -  [ProTV] TVManager (Screen B)] Video Error: PlayerError\n",
    );
    r.feed(
        &mut m,
        "2026.11.20 21:04:12 Log        -  [VideoTXL:AVPro-2] Video error event: PlayerError\n",
    );
    assert_eq!(failures(&r).len(), 0);
    assert!(!hint(&r).should_prefer_og(SOURCE));
    assert!(
        r.file_log()
            .contains("[vrclog] world player error with no open video")
    );
}

#[test]
fn a_media_error_outside_protv_now_playing_is_not_a_player_error() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(
        &mut m,
        &format!(
            "[AVProVideo] Opening {PLAYBACK}\n2026.11.20 21:04:11 Log        -  [Chat] {{\"type\":\"mediaError\"}}\n"
        ),
    );
    assert_eq!(failures(&r).len(), 0);
}

const RELAY_UPSTREAM: &str = "https://cdn.example.com/stream/index.m3u8";
const RELAY_PATH: &str = "/play/relay-7/index.m3u8";

fn relay_stats(r: &Rig) -> Arc<Stats> {
    let stats = Arc::new(Stats::new(r.now()));
    let ux = r.out.ux();
    let request = vrcr_shared::wire::ResolveRequest {
        id: "req-relay".to_owned(),
        url: SOURCE.to_owned(),
        ..Default::default()
    };
    stats.playback.resolving(&request, r.now(), &ux);
    stats.playback.resolved(
        &request,
        &ResolveResponse {
            id: request.id.clone(),
            action: action::RESOLVED.to_owned(),
            url: Some(RELAY_UPSTREAM.to_owned()),
            ..ResolveResponse::default()
        },
        vrcr_watchdog::diagnostics::ResolveDelivery {
            cached: false,
            retried: false,
            bytes: 0,
        },
        r.now(),
        &ux,
    );
    stats
        .playback
        .begin_request(RELAY_PATH, RELAY_UPSTREAM, "relay-req-1", r.now(), &ux);
    stats
}

#[test]
fn a_confirmed_relay_play_clears_the_failure_hint_and_its_strikes() {
    let r = Rig::new().with_cache();
    let stats = relay_stats(&r);
    assert_eq!(
        stats.playback.source_for_path(RELAY_PATH).as_deref(),
        Some(SOURCE)
    );
    assert_eq!(
        stats
            .playback
            .source_for_path("/play/relay-unknown/index.m3u8"),
        None
    );
    let mut deps = r.deps(AudioCheck::new(AudioState::Ok, Some("Speakers")));
    deps.stats = Some(Arc::clone(&stats));
    let mut m = r.monitor_with(deps);
    hint(&r).record_load_failure(SOURCE);
    assert!(hint(&r).should_prefer_og(SOURCE));
    assert_eq!(hint(&r).strikes(SOURCE), 1);

    r.feed(
        &mut m,
        "[AVProVideo] Opening https://localhost.youtube.com:4443/play/relay-unknown/index.m3u8\n",
    );
    r.clock.advance(secs(2));
    r.feed(&mut m, "fwidth=1280 fheight=720\n");
    assert!(hint(&r).should_prefer_og(SOURCE));
    assert_eq!(hint(&r).strikes(SOURCE), 1);

    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening https://localhost.youtube.com:4443{RELAY_PATH}\n"),
    );
    r.clock.advance(secs(2));
    r.feed(&mut m, "fwidth=1280 fheight=720\n");
    assert!(!hint(&r).should_prefer_og(SOURCE));
    assert_eq!(hint(&r).strikes(SOURCE), 0);
    assert!(r.file_log().contains("failure hint cleared"));
}
