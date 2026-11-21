use vrcr_shared::source_url::{is_direct_media, normalize, same_source, youtube_video_id};

const WATCH: &str = "https://www.youtube.com/watch?v=abcdefghijk";

#[test]
fn youtube_links_normalize_to_one_watch_url() {
    for url in [
        "https://youtu.be/abcdefghijk",
        "https://youtu.be/abcdefghijk?si=tracking123",
        "https://www.youtube.com/watch?v=abcdefghijk&pp=ygUE&feature=share",
        "https://WWW.YouTube.com/watch?v=abcdefghijk#t=30",
        "https://m.youtube.com/shorts/abcdefghijk",
        "https://music.youtube.com/watch?v=abcdefghijk&list=RD",
        "https://www.youtube.com/live/abcdefghijk?feature=shared",
        "https://www.youtube.com/embed/abcdefghijk",
        "https://www.youtube-nocookie.com/embed/abcdefghijk",
        "  https://youtube.com/v/abcdefghijk  ",
    ] {
        assert_eq!(normalize(url), WATCH, "{url}");
    }
}

#[test]
fn youtube_video_id_rejects_malformed_ids_and_other_hosts() {
    assert_eq!(
        youtube_video_id("https://youtu.be/abc-_fghijk").as_deref(),
        Some("abc-_fghijk")
    );
    for url in [
        "https://youtu.be/short",
        "https://youtu.be/abcdefghijkl",
        "https://www.youtube.com/watch?v=abc$efghijk",
        "https://www.youtube.com/channel/abcdefghijk",
        "https://notyoutube.com/watch?v=abcdefghijk",
        "ftp://youtu.be/abcdefghijk",
        "not a url",
    ] {
        assert_eq!(youtube_video_id(url), None, "{url}");
    }
}

#[test]
fn other_links_lose_case_fragments_and_tracking_params_only() {
    assert_eq!(
        normalize("HTTPS://Live.CDN4.io/ttfcvr/index.m3u8#frag"),
        "https://live.cdn4.io/ttfcvr/index.m3u8"
    );
    assert_eq!(
        normalize("https://example.com/play?id=1&utm_source=x&UTM_Medium=y&fbclid=z&igshid=w"),
        "https://example.com/play?id=1"
    );
    assert_eq!(
        normalize("https://example.com/play?si=1&pp=2&feature=3"),
        "https://example.com/play"
    );
    assert_eq!(
        normalize("https://example.com/play?id=1&token=abc"),
        "https://example.com/play?id=1&token=abc"
    );
}

#[test]
fn non_web_or_unparseable_input_is_only_trimmed() {
    assert_eq!(normalize("  not a url  "), "not a url");
    assert_eq!(normalize("file:///C:/Video.MP4"), "file:///C:/Video.MP4");
    assert_eq!(normalize(""), "");
}

#[test]
fn direct_media_is_recognized_by_the_last_path_extension() {
    for url in [
        "https://live.cdn4.io/ttfcvr/index.m3u8",
        "https://stream.vrcdn.live/live/samohte.live.ts",
        "https://cdn.example.com/a/b/video.MP4?sig=abc",
        "https://cdn.example.com/dash/manifest.mpd",
    ] {
        assert!(is_direct_media(url), "{url}");
    }
    for url in [
        WATCH,
        "https://example.com/watch",
        "https://example.com/video.mp4/page",
        "https://twitch.tv/somebody",
        "not a url",
    ] {
        assert!(!is_direct_media(url), "{url}");
    }
}

#[test]
fn same_source_compares_normalized_forms() {
    assert!(same_source("https://youtu.be/abcdefghijk?si=x", WATCH));
    assert!(same_source(
        "https://Live.cdn4.io/ttfcvr/index.m3u8",
        "https://live.cdn4.io/ttfcvr/index.m3u8?utm_source=discord"
    ));
    assert!(!same_source(
        "https://live.cdn4.io/ttfcvr/index.m3u8",
        "https://live.cdn4.io/other/index.m3u8"
    ));
}
