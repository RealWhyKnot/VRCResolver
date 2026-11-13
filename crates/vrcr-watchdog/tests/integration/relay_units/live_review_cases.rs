use super::*;
use std::borrow::Cow;
use vrcr_watchdog::relay::live::{MAX_INPUT_BYTES, MediaInfo, trim};

fn generated(count: usize, seconds: &str) -> String {
    let mut body = "#EXTM3U\n#EXT-X-TARGETDURATION:12\n#EXT-X-MEDIA-SEQUENCE:500\n#EXT-X-DISCONTINUITY-SEQUENCE:3\n".to_owned();
    for segment in 0..count {
        writeln!(body, "#EXTINF:{seconds},\nseg_{segment:06}.ts").expect("fixture");
    }
    body
}

struct TimedSink {
    started: tokio::time::Instant,
    first: Option<Duration>,
    bytes: Vec<u8>,
}

impl manifest::ManifestSink for TimedSink {
    async fn write(&mut self, chunk: Bytes) -> std::io::Result<()> {
        self.first.get_or_insert(self.started.elapsed());
        self.bytes.extend_from_slice(&chunk);
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn live_streaming_budget_limits_wait_and_bypasses_typed_playlists_immediately() {
    use futures_util::StreamExt;
    for marker in [
        "",
        "#EXT-X-PLAYLIST-TYPE:VOD\n",
        "#EXT-X-PLAYLIST-TYPE:EVENT\n",
    ] {
        let body = generated(500, "5")
            .replace("#EXT-X-MEDIA-SEQUENCE:500", "#EXT-X-MEDIA-SEQUENCE:0")
            .replace("#EXTM3U\n", &format!("#EXTM3U\n{marker}"));
        let tail = futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(Bytes::from_static(b"#EXT-X-ENDLIST\n"))
        });
        let input = futures_util::stream::iter([Ok(Bytes::from(body.clone()))])
            .chain(tail)
            .boxed();
        let mut sink = TimedSink {
            started: tokio::time::Instant::now(),
            first: None,
            bytes: Vec::new(),
        };
        let result =
            manifest::localize_relay_stream(&policy(), input, &mut sink, MAX_MANIFEST_BYTES)
                .await
                .expect("relay");
        assert_eq!(
            sink.first,
            Some(if marker.is_empty() {
                Duration::from_secs(1)
            } else {
                Duration::ZERO
            })
        );
        assert_eq!(result.media.expect("media").kept, 500);
        assert_eq!(
            String::from_utf8(sink.bytes).expect("text"),
            localize(&policy(), &format!("{body}#EXT-X-ENDLIST\n"))
        );
    }
}

#[tokio::test(start_paused = true)]
async fn live_streaming_byte_budget_flushes_without_waiting_for_the_tail() {
    use futures_util::StreamExt;
    let mut body = generated(500, "5");
    body.push('#');
    body.push_str(&"x".repeat(MAX_INPUT_BYTES));
    body.push('\n');
    let tail = futures_util::stream::once(async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(Bytes::from_static(b"#EXT-X-ENDLIST\n"))
    });
    let input = futures_util::stream::iter([Ok(Bytes::from(body))])
        .chain(tail)
        .boxed();
    let mut sink = TimedSink {
        started: tokio::time::Instant::now(),
        first: None,
        bytes: Vec::new(),
    };
    let result = manifest::localize_relay_stream(&policy(), input, &mut sink, MAX_MANIFEST_BYTES)
        .await
        .expect("relay");
    assert_eq!(sink.first, Some(Duration::ZERO));
    assert_eq!(result.media.expect("media").kept, 500);
    assert_eq!(result.media.expect("media").untrimmed, "size limit");
}

#[tokio::test]
async fn live_bom_crlf_trim_and_relay_keep_line_endings() {
    for ending in ["\n", "\r\n"] {
        for bom in ["", "\u{feff}"] {
            let body = format!("{bom}{}", generated(150, "5").replace('\n', ending));
            let (trimmed, info) = trim(&body);
            assert_eq!(info.expect("media").kept, 12);
            assert!(trimmed.starts_with(&format!("{bom}#EXTM3U{ending}")));
            assert!(trimmed.ends_with(&format!("seg_000149.ts{ending}")));
            assert_eq!(trim(&trimmed).0, trimmed);
            for size in [1, 4096] {
                let input = futures_util::stream::iter(
                    body.as_bytes()
                        .chunks(size)
                        .map(|chunk| Ok(Bytes::copy_from_slice(chunk))),
                );
                let mut output = Vec::new();
                let result = manifest::localize_relay_stream(
                    &policy(),
                    input,
                    &mut output,
                    MAX_MANIFEST_BYTES,
                )
                .await
                .expect("relay");
                assert_eq!(result.media.expect("media").kept, 12);
                let output = String::from_utf8(output).expect("text");
                if ending == "\r\n" {
                    assert!(!output.replace("\r\n", "").contains('\n'));
                } else {
                    assert!(!output.contains('\r'));
                }
            }
            let vod = format!("{body}#EXT-X-ENDLIST{ending}");
            assert!(matches!(trim(&vod).0, Cow::Borrowed(_)));
            assert_eq!(trim(&vod).0, vod);
        }
    }
}

#[test]
fn live_extreme_dates_durations_and_incomplete_lines_are_safe() {
    for date in [
        "0000-01-01T00:00:00Z",
        "99999-01-01T00:00:00Z",
        "9999-12-31T23:59:59Z",
        "2026-12-31T23:59:60Z",
        "2026-01-01T00:00:00",
        "2026-01-01T00:00:00+99:99",
    ] {
        let body = generated(150, "5").replace(
            "#EXTM3U\n",
            &format!("#EXTM3U\n#EXT-X-PROGRAM-DATE-TIME:{date}\n"),
        );
        let _ = trim(&body);
    }
    for seconds in [
        "0",
        "-1",
        "NaN",
        "inf",
        "99999999999999999999999999999999999999",
    ] {
        let body = generated(150, seconds);
        assert_eq!(trim(&body).0, body);
    }
    let body = generated(150, "5").replace("#EXTINF:5,", "#EXTINF:5");
    assert_eq!(trim(&body).0, body);
    let body = format!("{}#EXTINF:5,", generated(150, "5"));
    assert_eq!(trim(&body).0, body);
}

#[test]
fn live_implicit_map_range_stays_byte_exact() {
    let body = generated(150, "5").replace(
        "#EXTM3U\n",
        "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"100\"\n",
    );
    assert!(matches!(trim(&body).0, Cow::Borrowed(_)));
    assert_eq!(trim(&body).0, body);
}

#[test]
fn live_hundred_thousand_segments_and_resource_limits() {
    let body = generated(100_000, "5");
    assert!(body.len() < MAX_INPUT_BYTES);
    let started = std::time::Instant::now();
    let (output, info) = trim(&body);
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(info.expect("media").kept, 12);
    assert!(output.contains("#EXT-X-MEDIA-SEQUENCE:100488\n"));
    assert!(output.ends_with("seg_099999.ts\n"));
    let too_many = generated(100_001, "5");
    assert!(matches!(trim(&too_many).0, Cow::Borrowed(_)));
    let too_large = format!("{}{}\n", generated(150, "5"), " ".repeat(MAX_INPUT_BYTES));
    assert!(matches!(trim(&too_large).0, Cow::Borrowed(_)));
}

#[test]
fn live_trim_notices_and_slow_hitches_are_bounded_per_stream() {
    let detector = HitchDetector::new();
    let (ux, output) = capture_ux();
    let log = Log::disabled();
    log.set_dev_console(true);
    let mut timing = sample(
        "GET",
        "/play/stream/index.m3u8",
        "https://example.test/private/path?secret=x",
        200,
        10,
        3000,
    );
    timing.media = Some(MediaInfo {
        bytes: 5_600_000,
        segments: 2880,
        kept: 12,
        target_ms: 5000,
        live: true,
        needs_trim: true,
        untrimmed: "",
    });
    timing.bytes_out = 31_000;
    for reload in 0..50 {
        detector.record(&timing, utc(reload * 5), &ux, &log);
    }
    let text = output.logs();
    assert_eq!(text.matches("playlist trimmed").count(), 1);
    assert_eq!(text.matches("slow-live-playlist-reload").count(), 5);
    assert!(!text.contains("large-media-playlist"));
    assert!(!text.contains("[warn] playlist trimmed"));
    assert!(!text.contains("private/path"));
    assert!(!text.contains("secret"));
    detector.record(&timing, utc(600), &ux, &log);
    assert_eq!(output.logs().matches("playlist trimmed").count(), 2);
    timing.media.as_mut().expect("media").bytes += 2_000_000;
    detector.record(&timing, utc(601), &ux, &log);
    timing.media.as_mut().expect("media").kept = 30;
    detector.record(&timing, utc(602), &ux, &log);
    assert_eq!(output.logs().matches("playlist trimmed").count(), 4);
    timing.local_path = "/play/another/index.m3u8".to_owned();
    detector.record(&timing, utc(603), &ux, &log);
    assert_eq!(output.logs().matches("playlist trimmed").count(), 5);
    timing.total_ms = 10;
    timing.bytes_out = 5_600_000;
    timing.media.as_mut().expect("media").kept = 2880;
    timing.media.as_mut().expect("media").untrimmed = "budget";
    for reload in 0..50 {
        detector.record(&timing, utc(1000 + reload * 5), &ux, &log);
    }
    assert_eq!(output.logs().matches("large-media-playlist").count(), 5);
    assert_eq!(output.logs().matches("untrimmed=budget").count(), 5);
}

fn tag_number(body: &str, tag: &str) -> usize {
    body.lines()
        .find_map(|line| line.strip_prefix(tag))
        .expect("tag")
        .parse()
        .expect("number")
}

#[test]
fn live_generated_playlists_preserve_tail_state_and_bypasses() {
    let mut seed = 0x5eed_u64;
    for count in 0..=3000 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let seconds =
            ["0.2", "0.4", "1", "2", "5", "12"][usize::try_from(seed % 6).expect("index")];
        let mut body = generated(0, seconds);
        if seed & 1 != 0 {
            body.push_str("#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x01\n");
        }
        if seed & 2 != 0 {
            body.push_str("#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"100@0\"\n");
        }
        if seed & 4 != 0 {
            body.push_str("#EXT-X-PROGRAM-DATE-TIME:2026-11-10T00:00:00.125Z\n");
        }
        let mut discontinuities = Vec::new();
        for segment in 0..count {
            let discontinuity = seed & 8 != 0 && segment % 23 == 0;
            discontinuities.push(discontinuity);
            if discontinuity {
                body.push_str("#EXT-X-DISCONTINUITY\n");
            }
            writeln!(body, "#EXTINF:{seconds},").expect("fixture");
            if seed & 16 != 0 {
                writeln!(
                    body,
                    "#EXT-X-BYTERANGE:10{}\nmedia.ts",
                    if segment == 0 { "@0" } else { "" }
                )
                .expect("fixture");
            } else {
                writeln!(body, "seg_{segment:06}.ts").expect("fixture");
            }
        }
        if seed & 32 != 0 {
            body = body.replace('\n', "\r\n");
        }
        let (output, _) = trim(&body);
        let kept = output.matches("#EXTINF:").count();
        assert!(
            kept >= count.min(12) && kept <= 120,
            "count={count} kept={kept}"
        );
        assert_eq!(
            output.lines().rfind(|line| !line.starts_with('#')),
            body.lines().rfind(|line| !line.starts_with('#'))
        );
        let removed = count - kept;
        assert_eq!(tag_number(&output, "#EXT-X-MEDIA-SEQUENCE:"), 500 + removed);
        assert_eq!(
            tag_number(&output, "#EXT-X-DISCONTINUITY-SEQUENCE:"),
            3 + discontinuities[..removed]
                .iter()
                .filter(|value| **value)
                .count()
        );
        assert_eq!(trim(&output).0, output);
        if count % 30 == 0 {
            for tag in [
                "#EXT-X-ENDLIST",
                "#EXT-X-PLAYLIST-TYPE:EVENT",
                "#EXT-X-PART-INF:PART-TARGET=0.2",
                "#EXT-X-SKIP:SKIPPED-SEGMENTS=1",
                "#EXT-X-DEFINE:NAME=\"x\",VALUE=\"y\"",
                "#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\",KEYFORMAT=\"other\"",
            ] {
                let bypass = format!("{body}{tag}\n");
                assert_eq!(trim(&bypass).0, bypass);
            }
        }
    }
}

#[tokio::test]
async fn live_empty_endlist_is_reported_as_finished() {
    let input = futures_util::stream::iter([Ok(Bytes::from_static(b"#EXTM3U\n#EXT-X-ENDLIST\n"))]);
    let mut output = Vec::new();
    let result = manifest::localize_relay_stream(&policy(), input, &mut output, MAX_MANIFEST_BYTES)
        .await
        .expect("playlist");
    let media = result.media.expect("ended media");
    assert!(!media.live);
    assert_eq!(media.segments, 0);
}

#[tokio::test]
async fn live_streamed_bodies_report_whether_the_window_needed_trimming() {
    for (count, seconds, expected) in [
        (12, "5", false),
        (30, "5", true),
        (30, "0.4", false),
        (121, "0.4", true),
    ] {
        let input = futures_util::stream::iter([Ok(Bytes::from(generated(count, seconds)))]);
        let mut output = Vec::new();
        let result = manifest::localize_stream(&policy(), input, &mut output, MAX_MANIFEST_BYTES)
            .await
            .expect("playlist");
        assert_eq!(result.media.expect("media").needs_trim, expected);
    }
}
