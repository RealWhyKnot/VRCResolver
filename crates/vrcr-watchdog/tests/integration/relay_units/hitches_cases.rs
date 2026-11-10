use super::*;

#[test]
fn hitch_golden_analyze() {
    let h = HitchDetector::new();
    let now = utc(1_790_769_600);
    let lazy = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000010.ts";
    assert!(
        h.analyze(&sample("POST", "/a/seg.ts", lazy, 500, 9000, 9000), now)
            .is_none()
    );
    assert!(
        h.analyze(
            &sample("GET", "/a/x.bin", "https://h/x.bin", 500, 9000, 9000),
            now
        )
        .is_none()
    );
    assert!(
        h.analyze(
            &sample("GET", "/a/seg.ts", "https://h/p/seg.ts", 200, 10, 20),
            now
        )
        .is_none()
    );
    assert!(
        h.analyze(
            &sample("get", "/a/m.m3u8", "https://h/p/m.m3u8", 200, 1999, 2499),
            now
        )
        .is_none()
    );

    let mut s = sample(
        "GET",
        "/a/seg.ts",
        "https://h/p/seg.ts?x=1",
        503,
        1500,
        2500,
    );
    s.lazy_wait_ms = 1000;
    s.failure = Some("timeout".to_owned());
    let d = h.analyze(&s, now).unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(d.kind, "segment");
    assert_eq!(
        d.reasons,
        "segment-http-503,segment-timeout,slow-upstream-headers,slow-segment-total,server-generation-wait"
    );
    assert_eq!(
        (
            d.stream_id.as_str(),
            d.segment,
            d.previous_segment,
            d.gap_ms,
            d.header_ms,
            d.total_ms
        ),
        ("-", -1, -1, -1, 1500, 2500)
    );

    let d = h
        .analyze(
            &sample("GET", "/a/seg.m4s", "not a url", 200, 10, 2500),
            now,
        )
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (d.kind, d.reasons.as_str()),
        ("segment", "slow-segment-total")
    );

    let mut s = sample(
        "GET",
        "/a/m.m3u8",
        "https://h/p/m.m3u8?q=1",
        404,
        2000,
        2500,
    );
    s.failure = Some("reset".to_owned());
    let d = h.analyze(&s, now).unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(d.kind, "manifest");
    assert_eq!(
        d.reasons,
        "manifest-http-404,manifest-reset,slow-manifest-headers,slow-manifest-total"
    );
    assert_eq!(d.stream_id, "-");

    let d = h
        .analyze(
            &sample("GET", "/a/index.mpd", "https://h/p/x", 200, 2000, 10),
            now,
        )
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (d.kind, d.reasons.as_str()),
        ("manifest", "slow-manifest-headers")
    );

    let d = h
        .analyze(&sample("GET", "/a/seg.ts", lazy, 200, 1600, 10), now)
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (
            d.kind,
            d.reasons.as_str(),
            d.stream_id.as_str(),
            d.segment,
            d.previous_segment,
            d.gap_ms
        ),
        ("segment", "slow-upstream-headers", "wk_s1", 10, -1, -1)
    );

    let skip = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000013.ts";
    let d = h
        .analyze(
            &sample("GET", "/a/seg.ts", skip, 200, 10, 10),
            now + Duration::from_secs(3),
        )
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (d.reasons.as_str(), d.previous_segment, d.gap_ms, d.segment),
        ("segment-skip", 10, 3000, 13)
    );

    let back = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000005.ts";
    let d = h
        .analyze(
            &sample("GET", "/a/seg.ts", back, 200, 10, 10),
            now + Duration::from_secs(4),
        )
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (d.reasons.as_str(), d.previous_segment),
        ("segment-backtrack", 13)
    );
}

#[test]
fn hitch_lazy_parse_golden() {
    for (url, expected) in [
        (
            "https://h/api/proxy/lazy-hls/wk_a/seg_000291.ts",
            Some(("wk_a", 291)),
        ),
        (
            "https://h/api/proxy/LAZY-HLS/wk_a/x/seg_7.ts?q=1",
            Some(("wk_a", 7)),
        ),
        ("https://h/api/proxy/lazy-hls/wk_a/index.m3u8", None),
        ("https://h/api/proxy/lazy-hls/seg_1.ts", None),
        ("relative/lazy-hls/wk_a/seg_1.ts", None),
        ("https://h/api/proxy/lazy-hls/wk_a/seg_.ts", None),
        ("https://h/api/proxy/lazy-hls/wk_a/seg_99999999999.ts", None),
    ] {
        let got = parse_lazy_segment(url);
        assert_eq!(
            got.as_ref().map(|(s, n)| (s.as_str(), *n)),
            expected,
            "{url}"
        );
    }
}

#[test]
fn hitch_flags_segment_retries_but_not_steady_pacing() {
    let h = HitchDetector::new();
    let now = utc(1_778_454_000);
    let s = sample(
        "GET",
        "/play/a/seg_000042.ts",
        "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000042.ts",
        206,
        20,
        40,
    );
    assert!(h.analyze(&s, now).is_none());
    let retry = h
        .analyze(&s, now + Duration::from_secs(2))
        .unwrap_or_else(|| panic!("retry"));
    assert!(retry.reasons.contains("segment-retry"));
    assert_eq!(retry.previous_segment, 42);
    assert!(h.analyze(&s, now + Duration::from_secs(40)).is_none());

    let h = HitchDetector::new();
    let mut t = utc(1_790_706_412);
    for seg in 3..12 {
        let s = sample(
            "GET",
            &format!("/play/c629a1455fd0/seg_{seg:06}.ts"),
            &format!(
                "https://proxy.whyknot.dev/api/proxy/lazy-hls/wk_fe89cc94bd139c30p/seg_{seg:06}.ts"
            ),
            206,
            70,
            100,
        );
        assert!(h.analyze(&s, t).is_none(), "segment {seg}");
        t += Duration::from_millis(7200);
    }
}

#[test]
fn hitch_record_writes_the_file_line_or_the_console_line() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log = Log::to_file(&dir.path().join("relay.log"));
    let (ux, out) = capture_ux();
    let h = HitchDetector::new();
    let mut s = sample(
        "GET",
        "/play/a/seg.ts",
        "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=secret",
        502,
        10,
        3000,
    );
    s.lazy_generator = Some("ffmpeg".to_owned());
    s.lazy_wait_ms = 5;
    s.failure = Some("body_idle_timeout".to_owned());
    h.record(&s, utc(1_790_769_600), &ux, &log);
    log.close();
    let text = fs::read_to_string(dir.path().join("relay.log")).unwrap_or_default();
    assert!(text.contains(
        "[relay][hitch] reasons=segment-http-502,segment-body_idle_timeout,slow-segment-total kind=segment stream=wk_abc segment=1 status=502 lazy=HIT lazy_wait_ms=5 lazy_generator=ffmpeg header_ms=10 total_ms=3000 bytes=100 failure=body_idle_timeout target=us1.vrcresolver.com"
    ), "{text}");
    assert!(!text.contains("secret"));
    assert_eq!(out.text(), "");

    let log = Log::disabled();
    log.set_dev_console(true);
    let (ux, out) = capture_ux();
    HitchDetector::new().record(&s, utc(1_790_769_600), &ux, &log);
    assert!(out.text().contains(
        "[hitch] reasons=segment-http-502,segment-body_idle_timeout,slow-segment-total kind=segment stream=wk_abc segment=1 status=502 lazy=HIT lazy_wait_ms=5 lazy_generator=ffmpeg header_ms=10 total_ms=3000 bytes=100 failure=body_idle_timeout"
    ), "{}", out.text());
}
