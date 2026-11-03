use super::*;

#[test]
fn manifest_golden_output_for_lf_and_crlf_at_every_chunk_size() {
    let expected = golden_expected();
    for nl in ["\n", "\r\n", "\r"] {
        let input = golden_input(nl);
        assert_eq!(localize(&policy(), &input), expected, "{nl:?}");
        for size in [1, 2, 3, 7, 64, 4096, input.len()] {
            let (out, r) = stream_localize(input.as_bytes(), size, MAX_MANIFEST_BYTES);
            assert_eq!(out, expected, "{nl:?} chunk={size}");
            assert!(r.changed);
            assert!(!r.exceeded);
            assert_eq!(
                r.bytes_out,
                u64::try_from(expected.len()).expect("length fits the byte counter")
            );
        }
    }
}

#[test]
fn manifest_stream_strips_a_utf8_bom_even_when_split() {
    let mut input = b"\xEF\xBB\xBF".to_vec();
    input.extend_from_slice(b"#EXTM3U\nseg1.ts\n");
    for size in [1, 2, 3, 4, 100] {
        let (out, r) = stream_localize(&input, size, MAX_MANIFEST_BYTES);
        assert_eq!(out, "#EXTM3U\nseg1.ts", "chunk={size}");
        assert!(!r.changed);
    }
    let (out, _) = stream_localize(b"\xEF\xBB\xBF", 1, MAX_MANIFEST_BYTES);
    assert_eq!(out, "");
    let (out, _) = stream_localize(b"\xEF\xBB", 1, MAX_MANIFEST_BYTES);
    assert_eq!(out, "\u{FFFD}");
}

#[test]
fn unchanged_manifest_is_reported_unchanged() {
    let (out, r) = stream_localize(b"#EXTM3U\n#EXTINF:2,\nseg1.ts\n\n", 5, MAX_MANIFEST_BYTES);
    assert!(!r.changed);
    assert_eq!(out, "#EXTM3U\n#EXTINF:2,\nseg1.ts\n");
}

#[test]
fn manifest_stream_stops_once_output_exceeds_the_cap() {
    let (out, r) = stream_localize(b"#EXTM3U\n#EXTINF:2,\nseg1.ts\nseg2.ts\n", 4, 12);
    assert!(r.exceeded);
    assert_eq!(out, "#EXTM3U\n#EXTINF:2,");
}

#[test]
fn manifest_rewrites_browser_stream_segments() {
    let upstream =
        "https://eu1.vrcresolver.com/api/stream/0123456789abcdef01234567/7.ts?rsedge=eu1";
    let out = localize(&policy(), &format!("#EXTM3U\n#EXTINF:4.000,\n{upstream}\n"));
    assert!(!out.contains(upstream));
    assert!(out.contains("proxy/"));
    assert!(out.contains("/7.ts?target="));
}

#[test]
fn manifest_rewrites_proxy_lines_into_targets_the_resolver_accepts() {
    let upstream = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8";
    let out = localize(
        &policy(),
        &format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\n{upstream}\n"),
    );
    assert!(!out.contains("https://us1.vrcresolver.com"));
    let line = out
        .lines()
        .find(|l| l.starts_with("proxy/"))
        .unwrap_or_else(|| panic!("{out}"));
    let (local, query) = line.split_once('?').unwrap_or_else(|| panic!("{line}"));
    assert!(local.ends_with("/index.m3u8"));
    let param = query
        .strip_prefix("target=")
        .unwrap_or_else(|| panic!("{query}"));
    let t = TargetResolver::new()
        .resolve(
            &policy(),
            &format!("/play/7f8a9b0c1d2e/{local}"),
            &format!("?{query}"),
            Some(param),
        )
        .unwrap_or_else(|| panic!("target"));
    assert_eq!(t.url, upstream);
}

#[test]
fn manifest_rewrites_quoted_uris_and_leaves_relative_lines() {
    let upstream = "https://eu1.vrcresolver.com/api/proxy/seg.ts?url=abc";
    let out = localize(
        &policy(),
        &format!(
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"{upstream}\"\n#EXTINF:4.000,\nseg_000001.ts\n"
        ),
    );
    assert!(!out.contains(upstream));
    assert!(out.contains("URI=\"proxy/"));
    assert!(out.contains("/seg.ts?target="));
    assert!(out.contains("\nseg_000001.ts"));
}

#[test]
fn manifest_gives_same_file_name_targets_distinct_namespaces() {
    let out = localize(
        &policy(),
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nhttps://us1.vrcresolver.com/api/proxy/a/index.m3u8?x=1\n#EXT-X-STREAM-INF:BANDWIDTH=2\nhttps://us1.vrcresolver.com/api/proxy/b/index.m3u8?x=2\n",
    );
    let locals: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("proxy/"))
        .map(|l| &l[..l.find("/index.m3u8").unwrap_or(0)])
        .collect();
    assert_eq!(locals.len(), 2);
    assert_ne!(locals[0], locals[1]);
}

#[test]
fn manifest_rewrites_urls_inside_xml_text() {
    let upstream = "https://eu1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.m4s";
    let out = localize(
        &policy(),
        &format!("<MPD><Period><BaseURL>{upstream}</BaseURL></Period></MPD>"),
    );
    assert!(!out.contains(upstream));
    assert!(out.contains("<BaseURL>proxy/"));
    assert!(out.contains("/seg_000001.m4s?target="));
}

#[test]
fn manifest_leaves_third_party_urls_alone() {
    let m = "#EXTM3U\n#EXTINF:4.000,\nhttps://cdn.example.com/video/seg.ts";
    assert_eq!(localize(&policy(), m), m);
}

#[test]
fn manifest_html_decoding_covers_numeric_entities() {
    assert_eq!(
        manifest::html_decode("a&amp;b&#38;c&#x26;d&lt;"),
        "a&b&c&d<"
    );
    assert_eq!(manifest::html_decode("a&bogus;b&"), "a&bogus;b&");
    assert_eq!(manifest::html_decode("plain"), "plain");
}

#[test]
fn manifest_streams_a_large_playback_token_manifest() {
    let token = "A".repeat(900);
    let mut text = String::from("#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n");
    for i in 0..5000 {
        let _ = write!(
            text,
            "#EXTINF:2.000,\nhttps://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_{i:06}.ts?playback_id={token}\n"
        );
    }
    text.push_str("#EXT-X-ENDLIST\n");
    assert!(text.len() > 4 * 1024 * 1024);
    let (out, r) = stream_localize(text.as_bytes(), 16 * 1024, MAX_MANIFEST_BYTES);
    assert!(!r.exceeded);
    assert!(r.changed);
    assert!(!out.contains("https://us1.vrcresolver.com"));
    assert_eq!(
        out.lines().filter(|l| l.starts_with("proxy/")).count(),
        5000
    );
    assert!(out.ends_with("#EXT-X-ENDLIST"));
}

#[test]
fn manifest_stream_and_string_agree() {
    let m = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\nhttps://us1.vrcresolver.com/api/proxy/a/index.m3u8?x=1\n#EXTINF:2,\nhttps://eu1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=tok\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://eu1.vrcresolver.com/api/proxy/key.bin?clientId=c\"\nhttps://cdn.example.com/video/seg.ts\n";
    let (streamed, _) = stream_localize(m.as_bytes(), 9, MAX_MANIFEST_BYTES);
    assert_eq!(streamed, localize(&policy(), m));
}

#[test]
fn manifest_detection_only_matches_manifest_shapes() {
    for (local, target, media, expected) in [
        (
            "/play/a/manifest.m3u8",
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            "text/plain",
            true,
        ),
        (
            "/play/a/index.m3u8",
            "https://us1.vrcresolver.com/api/proxy/index.m3u8",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/manifest.mpd",
            "https://us1.vrcresolver.com/api/proxy/manifest.mpd",
            "application/dash+xml",
            true,
        ),
        (
            "/play/a/manifest.m3u8?token=abc",
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?token=abc",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/manifest.bin",
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            "application/octet-stream",
            true,
        ),
        (
            "/play/a/manifest",
            "https://us1.vrcresolver.com/api/proxy/manifest",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/manifest.mp4",
            "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=abc",
            "video/mp4",
            false,
        ),
        (
            "/play/a/MANIFEST.MP4",
            "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=abc",
            "video/mp4",
            false,
        ),
        (
            "/play/a/seg.ts",
            "https://us1.vrcresolver.com/api/proxy/seg.ts?url=abc",
            "video/mp2t",
            false,
        ),
        (
            "/play/a/manifest_archive/seg.ts",
            "https://us1.vrcresolver.com/api/proxy/seg.ts",
            "video/mp2t",
            false,
        ),
        (
            "/play/a/foo.ts",
            "https://example.test/playlist.ts",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/foo.ts",
            "https://example.test/x",
            "Application/XML; charset=utf-8",
            true,
        ),
        ("/play/a/foo.ts", "https://example.test/x", "text/xml", true),
        (
            "/play/a/foo.ts",
            "https://example.test/x",
            "application/xml+other",
            false,
        ),
    ] {
        assert_eq!(
            is_likely_manifest(local, target, Some(media)),
            expected,
            "{local} {media}"
        );
    }
    assert!(!is_likely_manifest(
        "/play/a/seg.ts",
        "https://h/x.ts",
        None
    ));
}

#[test]
fn relay_kind_splits_segments_manifests_and_the_rest() {
    let kind =
        |local: &str, target: &str| hitch::relay_kind(&sample("GET", local, target, 200, 1, 2));
    assert_eq!(
        kind(
            "/a/x",
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000010.ts"
        ),
        RelayKind::Segment
    );
    assert_eq!(
        kind("/a/x", "https://h/p/chunk.m4s?sig=1"),
        RelayKind::Segment
    );
    assert_eq!(kind("/a/seg.ts", "https://h/p/data"), RelayKind::Segment);
    assert_eq!(
        kind("/a/x", "https://h/p/index.m3u8?sig=1"),
        RelayKind::Manifest
    );
    assert_eq!(
        kind("/a/manifest.mpd", "https://h/p/data"),
        RelayKind::Manifest
    );
    assert_eq!(kind("/a/x.bin", "https://h/x.bin"), RelayKind::Other);
}
