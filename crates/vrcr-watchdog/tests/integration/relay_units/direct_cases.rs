use vrcr_watchdog::relay::direct::{SlowStreams, origin_of};
use vrcr_watchdog::relay::live::MediaInfo;

use super::*;

const ORIGIN: &str = "https://91334ab1a2f2.j.cloudfront.hls.ttvnw.net/v1/segment/abc.ts";

fn proxied(origin: &str) -> String {
    format!(
        "https://proxy.vrcresolver.com/api/proxy/seg.ts?url={}&pt=tok",
        b64::encode(origin)
    )
}

fn live_manifest(play: &str, target_ms: u64, live: bool) -> TimingSample {
    let mut manifest = sample(
        "GET",
        &format!("/play/{play}/index.m3u8"),
        "https://proxy.vrcresolver.com/api/proxy/seg.m3u8?url=bWFuaWZlc3Q",
        200,
        40,
        60,
    );
    manifest.media = Some(MediaInfo {
        target_ms,
        live,
        ..MediaInfo::default()
    });
    manifest
}

fn segment(play: &str, total: i64) -> TimingSample {
    sample(
        "GET",
        &format!("/play/{play}/seg.ts"),
        &proxied(ORIGIN),
        200,
        50,
        total,
    )
}

fn watching(play: &str, ux: &Ux) -> SlowStreams {
    let slow = SlowStreams::default();
    slow.observe(&live_manifest(play, 2000, true), ux);
    slow
}

#[test]
fn origin_is_decoded_from_a_server_segment_url() {
    assert_eq!(
        origin_of(&proxied(ORIGIN)).map(String::from),
        Some(ORIGIN.to_owned())
    );
}

#[test]
fn origin_is_refused_for_anything_but_a_public_https_segment() {
    let refused = [
        "https://proxy.vrcresolver.com/api/proxy/seg.m3u8?url=aHR0cHM6Ly9hLmIvYy5tM3U4".to_owned(),
        "https://proxy.vrcresolver.com/api/other/seg.ts?url=aHR0cHM6Ly9hLmIvYy50cw".to_owned(),
        proxied("http://video-edge.hls.ttvnw.net/a.ts"),
        proxied("https://cdn.example.com/a.ts"),
        proxied("https://evilttvnw.net/a.ts"),
        proxied("https://ttvnw.net.example.com/a.ts"),
        proxied("https://192.168.1.10/a.ts"),
        proxied("https://localhost/a.ts"),
        proxied("https://proxy.vrcresolver.com/a.ts"),
        "https://proxy.vrcresolver.com/api/proxy/seg.ts?url=%%%".to_owned(),
        "https://proxy.vrcresolver.com/api/proxy/seg.ts".to_owned(),
    ];
    for target in refused {
        assert_eq!(origin_of(&target), None, "{target}");
    }
}

#[test]
fn three_slow_segments_out_of_four_switch_the_video_to_direct() {
    let (ux, out) = capture_ux();
    let slow = watching("live1", &ux);
    for total in [2600, 900, 2400] {
        assert!(!slow.observe(&segment("live1", total), &ux));
        assert_eq!(slow.direct_origin("live1", &proxied(ORIGIN)), None);
    }
    assert!(slow.observe(&segment("live1", 3100), &ux));
    assert!(!slow.observe(&segment("live1", 3100), &ux));
    assert_eq!(
        slow.direct_origin("live1", &proxied(ORIGIN))
            .map(String::from),
        Some(ORIGIN.to_owned())
    );
    assert_eq!(
        out.logs()
            .matches("switching it to direct playback")
            .count(),
        1
    );
    assert_eq!(slow.direct_origin("other", &proxied(ORIGIN)), None);
}

#[test]
fn segments_that_keep_up_never_switch() {
    let (ux, out) = capture_ux();
    let slow = watching("live1", &ux);
    for total in [1900, 2600, 1200, 2100, 800, 2700, 1500, 1999] {
        slow.observe(&segment("live1", total), &ux);
    }
    assert_eq!(slow.direct_origin("live1", &proxied(ORIGIN)), None);
    assert!(!out.logs().contains("direct playback"));
}

#[test]
fn failed_segments_count_as_slow_but_player_hangups_do_not() {
    let (ux, _out) = capture_ux();
    let slow = watching("live1", &ux);
    for _ in 0..4 {
        let mut gone = segment("live1", 9000);
        gone.failure = Some("client_disconnect".to_owned());
        slow.observe(&gone, &ux);
    }
    assert_eq!(slow.direct_origin("live1", &proxied(ORIGIN)), None);
    let mut timed_out = segment("live1", 30_000);
    timed_out.failure = Some("upstream_timeout".to_owned());
    slow.observe(&timed_out, &ux);
    let mut bad_gateway = segment("live1", 100);
    bad_gateway.status = 502;
    slow.observe(&bad_gateway, &ux);
    slow.observe(&segment("live1", 2500), &ux);
    assert!(slow.direct_origin("live1", &proxied(ORIGIN)).is_some());
}

#[test]
fn recorded_videos_and_unknown_videos_never_switch() {
    let (ux, _out) = capture_ux();
    let slow = SlowStreams::default();
    slow.observe(&live_manifest("vod1", 6000, false), &ux);
    for _ in 0..6 {
        slow.observe(&segment("vod1", 9000), &ux);
        slow.observe(&segment("unseen", 9000), &ux);
    }
    assert_eq!(slow.direct_origin("vod1", &proxied(ORIGIN)), None);
    assert_eq!(slow.direct_origin("unseen", &proxied(ORIGIN)), None);
}

#[test]
fn giving_up_on_direct_goes_back_through_the_server_until_it_is_slow_again() {
    let (ux, out) = capture_ux();
    let slow = watching("live1", &ux);
    for _ in 0..3 {
        slow.observe(&segment("live1", 4000), &ux);
    }
    assert!(slow.direct_origin("live1", &proxied(ORIGIN)).is_some());
    slow.give_up("live1", &ux);
    slow.give_up("live1", &ux);
    assert_eq!(slow.direct_origin("live1", &proxied(ORIGIN)), None);
    assert_eq!(
        out.logs().matches("going back through the server").count(),
        1
    );
    for _ in 0..2 {
        slow.observe(&segment("live1", 4000), &ux);
    }
    assert_eq!(slow.direct_origin("live1", &proxied(ORIGIN)), None);
    slow.observe(&segment("live1", 4000), &ux);
    assert!(slow.direct_origin("live1", &proxied(ORIGIN)).is_some());
}

#[test]
fn a_direct_video_that_is_slow_too_goes_back_through_the_server() {
    let (ux, out) = capture_ux();
    let slow = watching("live1", &ux);
    for _ in 0..3 {
        slow.observe(&segment("live1", 4000), &ux);
    }
    assert!(slow.direct_origin("live1", &proxied(ORIGIN)).is_some());
    for total in [3000, 1000, 3500, 3200] {
        assert!(slow.direct_origin("live1", &proxied(ORIGIN)).is_some());
        slow.observe(&segment("live1", total), &ux);
    }
    assert_eq!(slow.direct_origin("live1", &proxied(ORIGIN)), None);
    assert_eq!(out.logs().matches("is slow too").count(), 1);
}
