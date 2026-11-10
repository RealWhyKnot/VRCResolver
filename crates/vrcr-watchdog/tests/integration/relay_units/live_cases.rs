use super::*;
use std::borrow::Cow;
use vrcr_watchdog::relay::live::{MediaInfo, trim};

fn playlist(count: usize, seconds: &str) -> String {
    let mut body =
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:100\n"
            .to_owned();
    for index in 0..count {
        writeln!(body, "#EXTINF:{seconds},\nseg_{index:06}.ts").expect("playlist");
    }
    body
}

#[test]
fn live_tail_meets_duration_and_count_with_a_hard_cap() {
    for (seconds, kept) in [("5", 12), ("2", 30), ("1", 60), ("0.4", 120), ("10", 12)] {
        let body = playlist(2880, seconds);
        let (output, info) = trim(&body);
        let info = info.expect("media info");
        assert_eq!((info.segments, info.kept), (2880, kept), "{seconds}");
        assert_eq!(output.matches("#EXTINF:").count(), kept);
        assert!(output.contains(&format!("#EXT-X-MEDIA-SEQUENCE:{}\n", 100 + 2880 - kept)));
        assert!(output.ends_with("seg_002879.ts\n"));
        let (again, _) = trim(&output);
        assert!(matches!(again, Cow::Borrowed(_)));
    }
    let body = playlist(20, "2");
    assert!(matches!(trim(&body).0, Cow::Borrowed(_)));
}

#[test]
fn live_trim_leaves_ineligible_playlists_byte_exact() {
    let body = playlist(150, "5").replace('\n', "\r\n");
    for tag in [
        "#EXT-X-ENDLIST",
        "#EXT-X-PLAYLIST-TYPE:VOD",
        "#EXT-X-PLAYLIST-TYPE:EVENT",
        "#EXT-X-PART:DURATION=0.4,URI=\"part.ts\"",
        "#EXT-X-PART-INF:PART-TARGET=0.4",
        "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part.ts\"",
        "#EXT-X-RENDITION-REPORT:URI=\"other.m3u8\"",
        "#EXT-X-SKIP:SKIPPED-SEGMENTS=5",
        "#EXT-X-DEFINE:NAME=\"token\",VALUE=\"x\"",
        "#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\",KEYFORMAT=\"other\"",
        "#EXT-X-STREAM-INF:BANDWIDTH=1000",
        "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio.m3u8\"",
        "#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=1000,URI=\"iframe.m3u8\"",
    ] {
        let input = format!("{body}{tag}\r\n");
        let (output, _) = trim(&input);
        assert!(matches!(output, Cow::Borrowed(_)), "{tag}");
        assert_eq!(output, input);
    }
    for count in [1, 11, 12] {
        let body = playlist(count, "5");
        assert_eq!(trim(&body).0, body);
    }
    let master = "#EXTM3U\r\n#EXT-X-STREAM-INF:BANDWIDTH=1000\r\na.m3u8\r\n";
    assert_eq!(trim(master).0, master);
}

#[test]
fn live_trim_carries_keys_maps_ranges_dates_and_discontinuities() {
    let mut body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:100\n#EXT-X-DISCONTINUITY-SEQUENCE:4\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-START:TIME-OFFSET=-15\n#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=NO\n#EXT-X-PROGRAM-DATE-TIME:2026-11-10T12:00:00.125+02:00\n".to_owned();
    for index in 0..20 {
        if index == 2 || index == 7 || index == 8 {
            body.push_str("#EXT-X-DISCONTINUITY\n");
        }
        if index == 5 {
            body.push_str(
                "#EXT-X-KEY:METHOD=AES-128,URI=\"map.key\",KEYFORMAT=\"identity\",IV=0x01\n",
            );
        }
        if index == 6 {
            body.push_str("#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"100@0\"\n");
        }
        if index == 7 {
            body.push_str("#EXT-X-KEY:METHOD=AES-128,URI=\"segment.key\"\n");
        }
        writeln!(
            body,
            "#EXTINF:5,\n#EXT-X-BYTERANGE:10{}\nmedia.ts",
            if index == 0 { "@0" } else { "" }
        )
        .expect("playlist");
    }
    let (output, _) = trim(&body);
    assert!(output.contains("#EXT-X-MEDIA-SEQUENCE:108\n"));
    assert!(output.contains("#EXT-X-DISCONTINUITY-SEQUENCE:6\n"));
    assert_eq!(output.matches("#EXT-X-DISCONTINUITY\n").count(), 1);
    assert!(output.contains("#EXT-X-BYTERANGE:10@80\n"));
    assert!(output.contains("#EXT-X-PROGRAM-DATE-TIME:2026-11-10T10:00:40.125000000Z\n"));
    let key = output.find("URI=\"map.key\"").expect("map key");
    let map = output.find("#EXT-X-MAP:").expect("map");
    let segment_key = output.find("URI=\"segment.key\"").expect("segment key");
    assert!(key < map && map < segment_key);
    for header in [
        "#EXT-X-VERSION:7",
        "#EXT-X-TARGETDURATION:5",
        "#EXT-X-INDEPENDENT-SEGMENTS",
        "#EXT-X-START:TIME-OFFSET=-15",
        "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=NO",
    ] {
        assert!(output.contains(header));
    }
}

#[test]
fn live_trim_keeps_boundary_changes_and_subsecond_dates() {
    let body = playlist(140, "0.4")
        .replace("#EXTINF:0.4,\nseg_000000.ts", "#EXT-X-KEY:METHOD=AES-128,URI=\"old.key\"\n#EXT-X-MAP:URI=\"old.mp4\"\n#EXT-X-PROGRAM-DATE-TIME:2026-11-10T00:00:00.123456789Z\n#EXTINF:0.4,\nseg_000000.ts")
        .replace("#EXTINF:0.4,\nseg_000020.ts", "#EXT-X-KEY:METHOD=NONE\n#EXT-X-MAP:URI=\"new.mp4\"\n#EXTINF:0.4,\nseg_000020.ts");
    let (output, _) = trim(&body);
    assert!(!output.contains("old.key"));
    assert!(!output.contains("old.mp4"));
    assert_eq!(output.matches("#EXT-X-KEY:METHOD=NONE").count(), 1);
    assert!(output.contains("#EXT-X-PROGRAM-DATE-TIME:2026-11-10T00:00:08.123456789Z"));
    let body = body.replace(
        "#EXT-X-KEY:METHOD=NONE",
        "#EXT-X-PROGRAM-DATE-TIME:2026-11-11T00:00:00Z\n#EXT-X-KEY:METHOD=NONE",
    );
    assert!(trim(&body).0.contains("2026-11-11T00:00:00"));
}

#[test]
fn live_trim_rejects_malformed_state_anywhere() {
    let body = playlist(150, "5");
    for broken in [
        body.replace("#EXTINF:5,", "#EXTINF:NaN,"),
        body.replace("#EXTINF:5,", "#EXTINF:-5,"),
        body.replace("#EXTINF:5,", "#EXTINF:0,"),
        body.replace("#EXT-X-TARGETDURATION:5", "#EXT-X-TARGETDURATION:0"),
        body.replace("#EXT-X-VERSION:7", "#EXT-X-VERSION:garbage"),
        body.replace(
            "#EXT-X-MEDIA-SEQUENCE:100",
            "#EXT-X-MEDIA-SEQUENCE:18446744073709551615",
        ),
        format!("{body}#EXTINF:5,\n"),
        format!("{body}orphan.ts\n"),
        format!("{body}#EXT-X-KEY:METHOD=AES-128,URI=\"unterminated\n"),
        body.replace(
            "#EXTINF:5,\nseg_000149.ts",
            "#EXT-X-BYTERANGE:10\n#EXTINF:5,\nseg_000149.ts",
        ),
        body.replace(
            "#EXTINF:5,\nseg_000149.ts",
            "#EXT-X-PROGRAM-DATE-TIME:2026-02-30T12:00:00Z\n#EXTINF:5,\nseg_000149.ts",
        ),
    ] {
        assert!(matches!(trim(&broken).0, Cow::Borrowed(_)));
    }
}

#[tokio::test]
async fn live_localizer_preserves_bypassed_output_across_chunk_boundaries() {
    for body in [
        format!("{}#EXT-X-ENDLIST\n", playlist(150, "5")),
        playlist(150, "5").replace("#EXTM3U", "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT"),
        golden_input("\r\n"),
        playlist(10, "5"),
        playlist(150, "5").replace("#EXTINF:5,", "#EXTINF:NaN,"),
        "<MPD>https://us1.vrcresolver.com/api/proxy/video.mp4</MPD>\n".to_owned(),
    ] {
        for size in [1, 7, 4096] {
            let input = futures_util::stream::iter(
                body.as_bytes()
                    .chunks(size)
                    .map(|chunk| Ok(Bytes::copy_from_slice(chunk))),
            );
            let mut output = Vec::new();
            manifest::localize_relay_stream(&policy(), input, &mut output, MAX_MANIFEST_BYTES)
                .await
                .expect("localized");
            assert_eq!(
                String::from_utf8(output).expect("text"),
                localize(&policy(), &body)
            );
        }
    }
    let body = playlist(150, "2");
    for size in [1, 7, 4096] {
        let input = futures_util::stream::iter(
            body.as_bytes()
                .chunks(size)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk))),
        );
        let mut output = Vec::new();
        let result =
            manifest::localize_relay_stream(&policy(), input, &mut output, MAX_MANIFEST_BYTES)
                .await
                .expect("localized");
        assert_eq!(result.media.expect("media").kept, 30);
        assert_eq!(result.bytes_in, u64::try_from(body.len()).expect("length"));
        assert_eq!(
            String::from_utf8(output).expect("text"),
            localize(&policy(), &trim(&body).0)
        );
    }
}

#[test]
fn live_trim_garbage_never_panics() {
    let mut seed = 17_u64;
    for length in 0..1000 {
        let bytes: Vec<u8> = (0..length)
            .map(|_| {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                seed.to_le_bytes()[4]
            })
            .collect();
        let garbage = String::from_utf8_lossy(&bytes);
        let _ = trim(&garbage);
        let _ = trim(&format!(
            "{}#EXT-X-PROGRAM-DATE-TIME:{garbage}\n",
            playlist(15, "5")
        ));
        let _ = trim(&format!("{}#EXT-X-KEY:{garbage}\n", playlist(15, "5")));
    }
}

#[test]
fn live_trim_keeps_overlapping_date_ranges_and_discards_expired_cues() {
    let body = playlist(20, "5").replace(
        "#EXT-X-MEDIA-SEQUENCE:100\n",
        concat!(
            "#EXT-X-MEDIA-SEQUENCE:100\n",
            "#EXT-X-PROGRAM-DATE-TIME:2026-11-10T00:00:00Z\n",
            "#EXT-X-DATERANGE:ID=\"expired\",START-DATE=\"2026-11-10T00:00:00Z\",DURATION=10\n",
            "#EXT-X-DATERANGE:ID=\"active\",START-DATE=\"2026-11-10T00:00:00Z\",DURATION=50\n",
            "#EXT-X-DATERANGE:ID=\"open\",START-DATE=\"2026-11-10T00:00:00Z\"\n",
            "#EXT-X-CUEPOINT:TYPE=\"test\",DURATION=10,TIME_OFFSET=0.0\n",
        ),
    );
    let (output, info) = trim(&body);
    assert_eq!(info.expect("media").kept, 12);
    assert!(!output.contains("expired"));
    assert!(!output.contains("CUEPOINT"));
    assert!(output.contains("ID=\"active\""));
    assert!(output.contains("ID=\"open\""));
}

#[test]
fn live_manifest_hitches_use_strict_size_and_duration_thresholds() {
    let detector = HitchDetector::new();
    let mut timing = sample(
        "GET",
        "/play/test/index.m3u8",
        "https://h/index.m3u8",
        200,
        10,
        1000,
    );
    timing.media = Some(MediaInfo {
        bytes: 256 * 1024,
        segments: 200,
        kept: 200,
        target_ms: 2000,
        live: true,
        needs_trim: true,
        untrimmed: "",
    });
    timing.bytes_out = 256 * 1024;
    assert!(detector.analyze(&timing, utc(0)).is_none());
    timing.total_ms += 1;
    timing.media.as_mut().expect("media").bytes += 1;
    timing.bytes_out += 1;
    let diagnostic = detector.analyze(&timing, utc(0)).expect("hitch");
    assert_eq!(
        diagnostic.reasons,
        "large-media-playlist,slow-live-playlist-reload"
    );
    let (ux, output) = capture_ux();
    let log = Log::disabled();
    log.set_dev_console(true);
    timing.media.as_mut().expect("media").kept = 30;
    assert_eq!(
        detector
            .analyze(&timing, utc(0))
            .expect("slow hitch")
            .reasons,
        "slow-live-playlist-reload"
    );
    detector.record(&timing, utc(0), &ux, &log);
    assert!(
        output
            .text()
            .contains("playlist-bytes=262145 segments=200 trimmed=true kept=30")
    );
    timing.media.as_mut().expect("media").live = false;
    timing.media.as_mut().expect("media").kept = 200;
    assert_eq!(
        detector
            .analyze(&timing, utc(0))
            .expect("size hitch")
            .reasons,
        "large-media-playlist"
    );
    timing.media = None;
    assert!(detector.analyze(&timing, utc(0)).is_none());
}
