use super::*;

#[test]
fn video_ids_come_from_every_youtube_link_shape() {
    let links = [
        "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
        "https://youtube.com/watch?feature=share&v=aqz-KE-bpKQ&t=10s",
        "http://m.youtube.com/watch?v=aqz-KE-bpKQ",
        "https://music.youtube.com/watch?v=aqz-KE-bpKQ&list=PL1",
        "https://youtu.be/aqz-KE-bpKQ?si=abc",
        "https://www.youtube.com/shorts/aqz-KE-bpKQ",
        "https://www.youtube.com/embed/aqz-KE-bpKQ?autoplay=1",
        "https://www.youtube.com/live/aqz-KE-bpKQ",
        "https://www.youtube-nocookie.com/embed/aqz-KE-bpKQ",
        "  https://WWW.YOUTUBE.COM/watch?v=aqz-KE-bpKQ  ",
    ];
    for link in links {
        assert_eq!(
            youtube::video_id(link).as_deref(),
            Some(VIDEO),
            "link {link}"
        );
    }
}

#[test]
fn other_links_have_no_video_id() {
    let links = [
        "",
        "not a url",
        "ftp://www.youtube.com/watch?v=aqz-KE-bpKQ",
        "https://example.com/watch?v=aqz-KE-bpKQ",
        "https://notyoutube.com/watch?v=aqz-KE-bpKQ",
        "https://www.youtube.com.evil.example/watch?v=aqz-KE-bpKQ",
        "https://www.youtube.com/watch",
        "https://www.youtube.com/watch?v=short",
        "https://www.youtube.com/watch?v=aqz-KE-bpKQ-toolong",
        "https://www.youtube.com/watch?v=aqz%2FKE-bpKQ",
        "https://www.youtube.com/playlist?list=PL1",
        "https://www.youtube.com/@channel",
        "https://www.youtube.com/shorts/",
        "https://youtu.be/",
        "https://localhost.youtube.com:4443/play/abc/video.mp4",
    ];
    for link in links {
        assert_eq!(youtube::video_id(link), None, "link {link}");
    }
}

#[test]
fn visitor_data_is_read_from_the_guarded_reply() {
    assert_eq!(
        youtube::visitor_data(&visitor_reply()).as_deref(),
        Some(VISITOR)
    );
    for body in [
        "",
        ")]}'",
        "<html>consent</html>",
        ")]}'\n[[null,null,[[[null]]]]]",
        ")]}'\n[[null,null,[[[null,null,null,null,null,null,null,null,null,null,null,null,null,\"\"]]]]]",
        ")]}'\n[[null,null,[[[null,null,null,null,null,null,null,null,null,null,null,null,null,7]]]]]",
    ] {
        assert_eq!(youtube::visitor_data(body), None, "body {body}");
    }
}

#[test]
fn the_player_request_names_the_video_and_the_visitor() {
    let body: serde_json::Value = serde_json::from_str(&youtube::player_body(VIDEO, VISITOR))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(body["videoId"], VIDEO);
    assert_eq!(body["context"]["client"]["visitorData"], VISITOR);
    assert_eq!(body["context"]["client"]["clientName"], "VISIONOS");
    assert_eq!(
        body["context"]["client"]["clientVersion"],
        youtube::CLIENT_VERSION
    );
}

#[test]
fn only_a_playable_recorded_video_yields_a_manifest() {
    let reply = |status: &str, id: &str, is_live: bool, manifest: Option<&str>| {
        let mut value = serde_json::json!({
            "playabilityStatus": {"status": status},
            "videoDetails": {"videoId": id, "isLive": is_live},
            "streamingData": {},
        });
        if let Some(manifest) = manifest {
            value["streamingData"]["hlsManifestUrl"] = manifest.into();
        }
        value.to_string()
    };
    let manifest = Some("https://manifest.googlevideo.com/m.m3u8");
    let cases = [
        (
            reply("OK", VIDEO, false, manifest),
            Ok("https://manifest.googlevideo.com/m.m3u8".to_owned()),
        ),
        (
            reply("LOGIN_REQUIRED", VIDEO, false, manifest),
            Err(youtube::SIGN_IN_WALL),
        ),
        (reply("ERROR", VIDEO, false, manifest), Err("not_playable")),
        (
            reply("UNPLAYABLE", VIDEO, false, manifest),
            Err("not_playable"),
        ),
        (
            reply("OK", "zzzzzzzzzzz", false, manifest),
            Err("wrong_video"),
        ),
        (reply("OK", VIDEO, true, manifest), Err("live")),
        (reply("OK", VIDEO, false, None), Err("no_hls")),
        (reply("OK", VIDEO, false, Some("")), Err("no_hls")),
        ("{}".to_owned(), Err("not_playable")),
        ("<html>".to_owned(), Err("player_unreadable")),
    ];
    for (player, expected) in cases {
        assert_eq!(
            youtube::manifest_url(&player, VIDEO),
            expected,
            "reply {player}"
        );
    }
}

#[test]
fn the_best_h264_variant_is_picked_with_one_english_audio_track() {
    let picked = master::pick(MASTER, &base(), None).unwrap_or_else(|| panic!("nothing picked"));
    assert_eq!(picked.height, 1080);
    assert_eq!(picked.video_codec, "avc1.64002A");
    assert_eq!(picked.audio_codec.as_deref(), Some("mp4a.40.2"));
    assert_eq!(
        picked.variant_url,
        "https://manifest.googlevideo.com/api/manifest/hls_variant/x/v1080p60.m3u8"
    );
    assert_eq!(
        picked.manifest,
        "#EXTM3U
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MEDIA:URI=\"https://manifest.googlevideo.com/api/manifest/hls_variant/x/audio/en.m3u8\",TYPE=AUDIO,GROUP-ID=\"234\",LANGUAGE=\"en-US\",NAME=\"English, original\",DEFAULT=NO,AUTOSELECT=YES
#EXT-X-STREAM-INF:BANDWIDTH=5500000,CODECS=\"avc1.64002A,mp4a.40.2\",RESOLUTION=1920x1080,FRAME-RATE=60,AUDIO=\"234\"
https://manifest.googlevideo.com/api/manifest/hls_variant/x/v1080p60.m3u8
"
    );
    assert_eq!(
        picked.audio_url.as_deref(),
        Some("https://manifest.googlevideo.com/api/manifest/hls_variant/x/audio/en.m3u8")
    );
}

#[test]
fn the_height_limit_is_honoured() {
    let heights = [
        (Some(2160), Some(1080)),
        (Some(1080), Some(1080)),
        (Some(720), Some(720)),
        (Some(480), Some(360)),
        (Some(240), None),
        (Some(0), Some(1080)),
        (Some(-1), Some(1080)),
    ];
    for (limit, expected) in heights {
        assert_eq!(
            master::pick(MASTER, &base(), limit).map(|picked| picked.height),
            expected,
            "limit {limit:?}"
        );
    }
}

#[test]
fn the_default_track_wins_when_no_english_one_exists() {
    let master = MASTER.replace("LANGUAGE=\"en-US\"", "LANGUAGE=\"de\"");
    let picked = master::pick(&master, &base(), None).unwrap_or_else(|| panic!("nothing picked"));
    assert!(picked.manifest.contains("audio/fr.m3u8"), "{picked:?}");
    assert!(!picked.manifest.contains("audio/en.m3u8"), "{picked:?}");
}

#[test]
fn unusable_masters_pick_nothing() {
    let no_audio_group = MASTER.replace("GROUP-ID=\"234\"", "GROUP-ID=\"999\"");
    let no_resolution = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc1.64001F\"\nv.m3u8\n";
    let no_target =
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc1.64001F\",RESOLUTION=1x1\n";
    let odd_scheme = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc1.64001F\",RESOLUTION=1x1\nfile:///c:/v.m3u8\n";
    for text in [
        "",
        "<html>",
        VP9_ONLY,
        no_audio_group.as_str(),
        no_resolution,
        no_target,
        odd_scheme,
    ] {
        assert_eq!(master::pick(text, &base(), None), None, "master {text}");
    }
}

#[test]
fn a_muxed_variant_needs_no_audio_track() {
    let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc1.64001F,mp4a.40.2\",RESOLUTION=1280x720\n\nv.m3u8\n";
    let picked = master::pick(text, &base(), None).unwrap_or_else(|| panic!("nothing picked"));
    assert_eq!(picked.manifest.matches("#EXT-X-MEDIA").count(), 0);
    assert_eq!(picked.height, 720);
}

#[test]
fn the_first_segment_follows_the_first_duration_line() {
    let base = base();
    assert_eq!(
        master::first_segment(PLAYLIST, &base).map(String::from),
        Some("https://manifest.googlevideo.com/api/manifest/hls_variant/x/seg0.ts".to_owned())
    );
    for text in [
        "",
        "#EXTM3U\n",
        "#EXTM3U\nseg0.ts\n",
        "#EXTM3U\n#EXTINF:5,\n",
    ] {
        assert_eq!(master::first_segment(text, &base), None, "playlist {text}");
    }
}

#[test]
fn manifests_expire_and_the_store_stays_small() {
    let store = Manifests::default();
    let now = Instant::now();
    let token = store.insert_at("a".to_owned(), WATCH, now);
    assert_eq!(store.get_at(&token, now).as_deref(), Some("a"));
    assert_eq!(
        store
            .get_at(&token, now + MANIFEST_TTL - Duration::from_secs(1))
            .as_deref(),
        Some("a")
    );
    assert_eq!(store.get_at(&token, now + MANIFEST_TTL), None);
    assert_eq!(store.get_at("missing", now), None);
    assert_eq!(store.get_at("", now), None);

    let first = store.insert_at("first".to_owned(), WATCH, now);
    let mut last = String::new();
    for index in 0..MAX_MANIFESTS {
        last = store.insert_at(format!("m{index}"), WATCH, now);
    }
    assert_eq!(store.get_at(&first, now), None);
    assert_eq!(
        store.get_at(&last, now),
        Some(format!("m{}", MAX_MANIFESTS - 1))
    );
}

fn play_url(token: &str) -> String {
    format!("{RELAY}/play/local-{token}/master.m3u8")
}

#[test]
fn a_failed_play_gives_its_source_once_and_stops_being_served() {
    let store = Manifests::default();
    let now = Instant::now();
    let url = play_url(&store.insert_at("a".to_owned(), WATCH, now));
    assert_eq!(store.source(&url).as_deref(), Some(WATCH));
    assert_eq!(store.failed_at(&url, now).as_deref(), Some(WATCH));
    assert_eq!(store.failed_at(&url, now), None);
    assert_eq!(store.source(&url), None);
    assert!(!store.confirmed(&url));
    for other in [
        "",
        "not a url",
        "https://localhost.youtube.com:4443/play/abc/video.mp4",
        play_url("unknown").as_str(),
    ] {
        assert_eq!(store.failed_at(other, now), None, "url {other}");
        assert!(!store.confirmed(other), "url {other}");
    }
    assert!(!store.is_resting_at(now));
}

#[test]
fn failures_in_a_row_rest_the_step_for_a_while() {
    let store = Manifests::default();
    let now = Instant::now();
    let fail = |at: Instant| {
        let url = play_url(&store.insert_at("a".to_owned(), WATCH, at));
        assert_eq!(store.failed_at(&url, at).as_deref(), Some(WATCH));
    };
    for _ in 1..FAILURES_BEFORE_REST {
        fail(now);
    }
    assert!(!store.is_resting_at(now));
    let url = play_url(&store.insert_at("a".to_owned(), WATCH, now));
    assert!(store.confirmed(&url));
    for _ in 1..FAILURES_BEFORE_REST {
        fail(now);
    }
    assert!(!store.is_resting_at(now));
    fail(now);
    assert!(store.is_resting_at(now));
    assert!(store.is_resting_at(now + REST - Duration::from_secs(1)));
    assert!(!store.is_resting_at(now + REST));
    for _ in 1..FAILURES_BEFORE_REST {
        fail(now + REST);
    }
    assert!(!store.is_resting_at(now + REST));
}

#[test]
fn the_variant_line_is_rebuilt_from_known_attributes_only() {
    let text = "#EXTM3U\n#EXT-X-STREAM-INF:SUBTITLES=\"vtt\",VIDEO=\"v\",BANDWIDTH=9,CODECS=\"avc1.64001F,mp4a.40.2\",RESOLUTION=1280x720,FRAME-RATE=29.97,X-EXTRA=\"a,b\"\nv.m3u8\n";
    let picked = master::pick(text, &base(), None).unwrap_or_else(|| panic!("nothing picked"));
    assert_eq!(
        picked.manifest,
        "#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-STREAM-INF:BANDWIDTH=9,CODECS=\"avc1.64001F,mp4a.40.2\",RESOLUTION=1280x720,FRAME-RATE=29.97\nhttps://manifest.googlevideo.com/api/manifest/hls_variant/x/v.m3u8\n"
    );
    assert_eq!(picked.audio_url, None);
    let odd = text
        .replace("1280x720", "wide\"x720")
        .replace("29.97", "fast");
    let picked = master::pick(&odd, &base(), None);
    assert_eq!(picked, None);
}

#[test]
fn faults_in_a_row_rest_the_step_until_an_answer_clears_them() {
    let store = Manifests::default();
    let now = Instant::now();
    for _ in 1..FAULTS_BEFORE_REST {
        store.fault_at(now);
    }
    store.answered();
    for _ in 1..FAULTS_BEFORE_REST {
        store.fault_at(now);
    }
    assert!(!store.is_resting_at(now));
    store.fault_at(now);
    assert!(store.is_resting_at(now));
    assert!(!store.is_resting_at(now + REST));
}

#[test]
fn a_play_from_elsewhere_clears_the_failed_load_count() {
    let store = Manifests::default();
    let now = Instant::now();
    for _ in 0..3 {
        for _ in 1..FAILURES_BEFORE_REST {
            let url = play_url(&store.insert_at("a".to_owned(), WATCH, now));
            assert_eq!(store.failed_at(&url, now).as_deref(), Some(WATCH));
        }
        store.played_elsewhere();
    }
    assert!(!store.is_resting_at(now));
}

#[test]
fn only_local_play_addresses_are_recognised() {
    for url in [
        play_url("abc"),
        "http://localhost:8080/play/local-abc/master.m3u8".to_owned(),
        "  https://127.0.0.1:4443/play/local-abc/master.m3u8?x=1 ".to_owned(),
    ] {
        assert!(local::is_play_url(&url), "url {url}");
    }
    for url in [
        "",
        "not a url",
        "https://example.com/play/local-abc/master.m3u8",
        "https://localhost.youtube.com.evil.example/play/local-abc/master.m3u8",
        "https://localhost.youtube.com:4443/play/abc/master.m3u8",
        "https://localhost.youtube.com:4443/play/local-abc/video.mp4",
    ] {
        assert!(!local::is_play_url(url), "url {url}");
    }
}

#[test]
fn relay_paths_name_one_manifest() {
    assert_eq!(
        local::token("/play/local-abc123/master.m3u8"),
        Some("abc123")
    );
    for path in [
        "/play/local-/master.m3u8",
        "/play/local-abc123",
        "/play/local-abc123/",
        "/play/local-abc123/other.m3u8",
        "/play/local-abc123/x/master.m3u8",
        "/play/abc123/master.m3u8",
        "/local-abc123/master.m3u8",
    ] {
        assert_eq!(local::token(path), None, "path {path}");
    }
}
