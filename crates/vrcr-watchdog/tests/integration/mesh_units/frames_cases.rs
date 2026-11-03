use super::*;

#[test]
fn wire_deadline_keeps_a_half_second_margin() {
    let now = Instant::now();
    let cases: [(i64, i32); 5] = [(10_000, 9_500), (501, 1), (500, 0), (300, 0), (-250, 0)];
    for (offset, want) in cases {
        let deadline = if offset >= 0 {
            now + Duration::from_millis(offset.unsigned_abs())
        } else {
            now.checked_sub(Duration::from_millis(offset.unsigned_abs()))
                .unwrap_or(now)
        };
        assert_eq!(wire_deadline_for(deadline, now), want, "offset {offset}");
    }
}

#[test]
fn msgpack_resolved_decodes_every_field() {
    let bytes = encode(vec![
        s("resolved"),
        s("x9k2pq8r4mzj7vn3"),
        s("https://rr1.googlevideo.com/foo"),
        s("yt-dlp"),
        s("youtube-tv-combo"),
        s("mp4"),
        s("h264"),
        s("aac"),
        s("http"),
        Mp::from(2),
        Mp::from(12_345_678i64),
        s("2026-05-04T21:00:00Z"),
        Mp::from(1080),
    ]);
    let r = resolved(&bytes);
    assert_eq!(r.action, "resolved");
    assert_eq!(r.id, "x9k2pq8r4mzj7vn3");
    assert_eq!(r.url.as_deref(), Some("https://rr1.googlevideo.com/foo"));
    assert_eq!(r.engine.as_deref(), Some("yt-dlp"));
    assert_eq!(r.config.as_deref(), Some("youtube-tv-combo"));
    assert_eq!(r.container.as_deref(), Some("mp4"));
    assert_eq!(r.video_codec.as_deref(), Some("h264"));
    assert_eq!(r.audio_codec.as_deref(), Some("aac"));
    assert_eq!(r.protocol.as_deref(), Some("http"));
    assert_eq!(r.audio_channels, Some(2));
    assert_eq!(r.bytes_estimate, Some(12_345_678));
    assert_eq!(r.expires_at.as_deref(), Some("2026-05-04T21:00:00Z"));
    assert_eq!(r.resolved_height, Some(1080));
}

#[test]
fn msgpack_resolved_with_trailing_nils_decodes() {
    let bytes = [
        0x9D, 0xA1, b'R', 0xA1, b'I', 0xA1, b'U', 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0, 0xC0,
        0xC0, 0xC0,
    ];
    match decode_binary(&bytes) {
        Ok(BinaryFrame::Other(name)) => assert_eq!(name, "R"),
        other => panic!("{other:?}"),
    }
    let mut named = vec![s("resolved"), s("shortid"), s("https://x")];
    named.extend(std::iter::repeat_n(Mp::Nil, 10));
    let r = resolved(&encode(named));
    assert_eq!(r.id, "shortid");
    assert_eq!(r.engine, None);
    assert_eq!(r.audio_channels, None);
    assert_eq!(r.bytes_estimate, None);
}

#[test]
fn msgpack_resolved_tolerates_extra_trailing_fields() {
    let mut items = vec![s("resolved"), s("id1"), s("https://example.com")];
    items.extend(std::iter::repeat_n(Mp::Nil, 10));
    items.push(s(""));
    items.push(Mp::Nil);
    let r = resolved(&encode(items));
    assert_eq!(r.id, "id1");
    assert_eq!(r.url.as_deref(), Some("https://example.com"));
}

#[test]
fn msgpack_resolved_tolerates_a_short_array_from_an_older_server() {
    let mut items = vec![s("resolved"), s("id1"), s("https://example.com")];
    items.extend(std::iter::repeat_n(Mp::Nil, 9));
    let r = resolved(&encode(items));
    assert_eq!(r.id, "id1");
    assert_eq!(r.resolved_height, None);
}

#[test]
fn msgpack_fallback_tolerates_short_and_four_element_frames() {
    let r = fallback_frame(&encode(vec![
        s("fallback_native"),
        s("id1"),
        s("warp_down"),
    ]));
    assert_eq!(r.reason.as_deref(), Some("warp_down"));
    assert_eq!(r.public_message, None);
    assert_eq!(r.retry_after_ms, None);
    let r = fallback_frame(&[
        0x94, 0xAF, b'f', b'a', b'l', b'l', b'b', b'a', b'c', b'k', b'_', b'n', b'a', b't', b'i',
        b'v', b'e', 0xA1, b'I', 0xA1, b'R', 0xC0,
    ]);
    assert_eq!(r.id, "I");
    assert_eq!(r.retry_after_ms, None);
}

#[test]
fn msgpack_fallback_carries_retry_after_and_public_message() {
    let r = fallback_frame(&encode(vec![
        s("fallback_native"),
        s("id1"),
        s("discovery_in_progress"),
        Mp::Nil,
        Mp::from(8500),
    ]));
    assert_eq!(r.retry_after_ms, Some(8500));
    let r = fallback_frame(&encode(vec![
        s("fallback_native"),
        s("id1"),
        s("drm_detected"),
        s("Content is DRM-protected and can't be played."),
    ]));
    assert_eq!(
        r.public_message.as_deref(),
        Some("Content is DRM-protected and can't be played.")
    );
}

#[test]
fn msgpack_control_and_log_frames() {
    assert_eq!(
        decode_binary(&encode(vec![s("ping")])),
        Ok(BinaryFrame::Ping)
    );
    assert_eq!(
        decode_binary(&encode(vec![s("pong")])),
        Ok(BinaryFrame::Pong)
    );
    assert_eq!(
        decode_binary(&encode(vec![
            s("resolve_log"),
            s("id1"),
            s("trying youtube-tv-combo")
        ])),
        Ok(BinaryFrame::ResolveLog {
            id: "id1".to_owned(),
            message: "trying youtube-tv-combo".to_owned()
        })
    );
}

#[test]
fn msgpack_garbage_is_an_error() {
    assert!(decode_binary(&[]).is_err());
    assert!(decode_binary(&[0x90]).is_err());
    assert!(decode_binary(&[0xA1, b'x']).is_err());
    assert!(decode_binary(&encode(vec![Mp::from(5)])).is_err());
    assert!(decode_binary(&encode(vec![s("resolved"), Mp::from(1)])).is_err());
    assert!(
        decode_binary(&encode(vec![
            s("resolved"),
            s("i"),
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::Nil,
            Mp::from(i64::MAX)
        ]))
        .is_err()
    );
}

#[test]
fn contract_subprotocols_match() {
    let c = contract();
    let current = c["subprotocols"]["current"].as_str().unwrap_or_default();
    let legacy = c["subprotocols"]["legacy"].as_str().unwrap_or_default();
    assert_eq!(current, SUBPROTOCOL_V3);
    assert_eq!(
        c["protocol_version"].as_i64(),
        Some(i64::from(CLIENT_PROTOCOL_VERSION))
    );
    assert!(should_send_client_hello(Some(current)));
    assert!(!should_send_client_hello(Some(legacy)));
}

#[test]
fn contract_client_fallback_reasons_exist_on_the_server() {
    let server = strings(&contract(), "fallback_reasons");
    for reason in [
        fallback::ALL_CONFIGS_FAILED,
        fallback::EXTRACTOR_UNSUPPORTED,
        fallback::INTERNAL_ERROR,
        fallback::DISCOVERY_IN_PROGRESS,
        fallback::UNITY_UNSUPPORTED_FORMAT,
        fallback::WARP_DOWN,
    ] {
        assert!(
            server.iter().any(|r| r == reason),
            "{reason} missing from the server list"
        );
    }
}

#[test]
fn contract_playback_feedback_kinds_match() {
    assert_eq!(
        strings(&contract(), "playback_feedback_kinds"),
        sorted(&[
            feedback::LOAD_FAILURE,
            feedback::SILENT_STALL,
            feedback::PLAYING,
            feedback::RESOLVED_REJECTED,
            feedback::OG_FAILED,
            feedback::CACHE_PLAY,
        ])
    );
}

#[test]
fn contract_resolved_rejected_details_match() {
    assert_eq!(
        strings(&contract(), "resolved_rejected_details"),
        sorted(&[
            og_reason::AVPRO_INCOMPATIBLE,
            og_reason::RESOLVED_URL_REJECTED
        ])
    );
}

#[test]
fn contract_og_failed_details_match() {
    assert_eq!(
        strings(&contract(), "og_failed_details"),
        sorted(&OG_FAILED_DETAILS)
    );
    for detail in OG_FAILED_DETAILS {
        assert_eq!(og_failed_detail_for(Some(detail)), detail);
    }
    assert_eq!(
        og_failed_detail_for(Some("something_else")),
        OG_FAILED_DETAIL_UNKNOWN
    );
}

#[test]
fn contract_proxy_extensions_match() {
    assert_eq!(
        strings(&contract(), "proxy_path_extensions"),
        sorted(&ALLOWED_PATH_EXTENSIONS)
    );
}

#[test]
fn contract_client_needed_features_are_listed_by_the_server() {
    let c = contract();
    let needed = [
        feature::PLAYBACK_FEEDBACK,
        feature::PLAYBACK_FEEDBACK_V2,
        feature::WELCOME_HOSTS,
    ];
    assert_eq!(strings(&c, "client_required_features"), sorted(&needed));
    let server = strings(&c, "server_features");
    let welcome: WelcomeFrame =
        serde_json::from_value(json_sample(&c, action::WELCOME)).unwrap_or_else(|e| panic!("{e}"));
    let advertised = welcome.features.unwrap_or_default();
    for f in needed {
        assert!(server.iter().any(|s| s == f), "{f} not in server_features");
        assert!(
            advertised.iter().any(|s| s == f),
            "{f} not in the welcome sample"
        );
    }
}

#[test]
fn contract_msgpack_samples_match_the_json_samples() {
    let c = contract();
    let names: Vec<String> = c["msgpack_samples_hex"]
        .as_object()
        .map(|o| {
            let mut v: Vec<String> = o.keys().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        sorted(&[
            action::RESOLVED,
            action::FALLBACK_NATIVE,
            action::RESOLVE_LOG
        ])
    );

    let want = json_sample(&c, action::RESOLVED);
    let got = resolved(&msgpack_sample(&c, action::RESOLVED));
    assert_eq!(Some(got.action.clone()), text(&want, "action"));
    assert_eq!(Some(got.id.clone()), text(&want, "id"));
    assert_eq!(got.url, text(&want, "url"));
    assert_eq!(got.engine, text(&want, "engine"));
    assert_eq!(got.config, text(&want, "config"));
    assert_eq!(got.container, text(&want, "container"));
    assert_eq!(got.video_codec, text(&want, "video_codec"));
    assert_eq!(got.audio_codec, text(&want, "audio_codec"));
    assert_eq!(got.protocol, text(&want, "protocol"));
    assert_eq!(
        got.audio_channels.map(i64::from),
        int(&want, "audio_channels")
    );
    assert_eq!(got.bytes_estimate, int(&want, "bytes_estimate"));
    assert_eq!(got.expires_at, text(&want, "expires_at"));
    assert_eq!(
        got.resolved_height.map(i64::from),
        int(&want, "resolved_height")
    );
    assert!(got.resolved_height.is_some());

    let want = json_sample(&c, action::FALLBACK_NATIVE);
    let got = fallback_frame(&msgpack_sample(&c, action::FALLBACK_NATIVE));
    assert_eq!(Some(got.action.clone()), text(&want, "action"));
    assert_eq!(Some(got.id.clone()), text(&want, "id"));
    assert_eq!(got.reason, text(&want, "reason"));
    assert_eq!(got.public_message, text(&want, "public_message"));
    assert_eq!(
        got.retry_after_ms.map(i64::from),
        int(&want, "retry_after_ms")
    );
    assert!(got.retry_after_ms.is_some());

    let want = json_sample(&c, action::RESOLVE_LOG);
    match decode_binary(&msgpack_sample(&c, action::RESOLVE_LOG)) {
        Ok(BinaryFrame::ResolveLog { id, message }) => {
            assert_eq!(Some(id), text(&want, "id"));
            assert_eq!(Some(message), text(&want, "message"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn contract_deadlines_nest_with_the_server_window() {
    let c = contract();
    let margin = number(&c, "resolve_window", "deadline_margin_ms");
    let min_window = number(&c, "resolve_window", "min_ms");
    let max_window = number(&c, "resolve_window", "max_ms");
    let now = Instant::now();
    let mut violations = Vec::new();
    for budget in (2_000..=28_000).step_by(500) {
        let sent = budget - 1_000;
        let watchdog = effective_budget_ms(sent);
        let wire = wire_deadline_for(
            now + Duration::from_millis(u64::try_from(watchdog).unwrap_or(0)),
            now,
        );
        let server = (wire - margin).clamp(min_window, max_window);
        let line =
            format!("budget={budget} sent={sent} watchdog={watchdog} wire={wire} server={server}");
        if watchdog >= budget {
            violations.push(format!(
                "watchdog does not finish before the wrapper: {line}"
            ));
        }
        if budget >= 7_000 && server >= watchdog {
            violations.push(format!(
                "server window does not finish before the watchdog: {line}"
            ));
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn contract_fixture_is_a_copy_of_the_server_fixture() {
    let checkout =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../Deployed/vrcresolver-server/apex");
    if !checkout.is_dir() {
        return;
    }
    let server = checkout.join("crates/vrcr-protocol/tests/fixtures/mesh-contract.json");
    let server_copy = fs::read_to_string(&server).unwrap_or_else(|e| {
        panic!(
            "server checkout found but {} is unreadable: {e}",
            server.display()
        )
    });
    assert_eq!(
        server_copy.replace("\r\n", "\n"),
        CONTRACT.replace("\r\n", "\n")
    );
}

#[test]
fn contract_wire_constants_keep_their_names() {
    assert_eq!(wire::SUBPROTOCOL_V3, "vrcresolver-v3");
    assert_eq!(action::CLIENT_HELLO, "client_hello");
    assert_eq!(action::WELCOME_CACHED, "welcome_cached");
    assert_eq!(action::PROTOCOL_ERROR, "protocol_error");
    assert_eq!(action::RATE_LIMITED, "rate_limited");
    assert_eq!(action::PLAYBACK_QUALITY_UPDATE, "playback_quality_update");
    assert_eq!(fallback::RATE_LIMITED, "rate_limited");
    assert_eq!(fallback::PROTOCOL_ERROR, "protocol_error");
    assert_eq!(wire::field::RETRY_AFTER_SECONDS, "retryAfterSeconds");
    assert_eq!(wire::field::MESH_ACTION, "meshAction");
    assert_eq!(wire::format::PREFERENCE, ["msgpack", "json"]);
}
