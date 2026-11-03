use super::*;

#[test]
fn play_id_reads_the_second_segment() {
    assert_eq!(play_id("/play/abc123/manifest.m3u8"), Some("abc123"));
    assert_eq!(play_id("/play/abc123/proxy/9f/seg.ts"), Some("abc123"));
    assert_eq!(play_id("/play/abc123"), Some("abc123"));
    for bad in ["", "/play/", "/not-play/abc/manifest.m3u8", "/PLAY/abc/x"] {
        assert_eq!(play_id(bad), None, "{bad}");
    }
}

#[test]
fn wedge_check_ignores_urls_that_are_not_the_local_relay() {
    let a = RelayActivity::new();
    assert_eq!(
        a.wedged_port("https://vrcresolver.com/play/x/manifest.m3u8"),
        None
    );
    assert_eq!(
        a.wedged_port("https://localhost.youtube.com:5000/other/x"),
        None
    );
    assert_eq!(a.wedged_port("not a url"), None);
}

#[test]
fn wedge_check_flags_plays_the_relay_never_saw() {
    let a = RelayActivity::new();
    let url = "https://localhost.youtube.com:54321/play/deadbeef/manifest.m3u8?target=abc";
    assert_eq!(a.wedged_port(url), Some(54321));
    a.record("/play/deadbeef/proxy/1/seg.ts");
    assert_eq!(a.wedged_port(url), None);
    assert_eq!(
        a.wedged_port("http://LOCALHOST.YOUTUBE.COM/play/other/manifest.m3u8"),
        Some(80)
    );
}

#[test]
fn activity_trims_to_half_once_past_the_cap() {
    let a = RelayActivity::new();
    for i in 0..=512 {
        a.record(&format!("/play/p{i}/manifest.m3u8"));
    }
    assert_eq!(a.tracked(), 256);
    assert_eq!(
        a.wedged_port("https://localhost.youtube.com:1/play/p0/x"),
        Some(1)
    );
    assert_eq!(
        a.wedged_port("https://localhost.youtube.com:1/play/p512/x"),
        None
    );
    a.record("/nothing");
    assert_eq!(a.tracked(), 256);
}
