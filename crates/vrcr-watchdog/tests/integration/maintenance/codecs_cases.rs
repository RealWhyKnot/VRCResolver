use super::*;

#[test]
fn mf_decoder_present_short_circuits_with_no_spawns() {
    let mut env = Env::new();
    env.mf = Box::new(|_| true);
    let mut state = CodecState::default();
    let (dirty, seen) = run_codecs(&env, &mut state);
    assert!(dirty);
    assert!(seen[0].installed);
    assert_eq!(seen[0].detail, "mf");
    assert_eq!(env.appx_calls.load(Ordering::SeqCst), 0);
    assert!(lock(&env.winget_calls).is_empty());
    assert_eq!(state.codecs["paid-id"].status, "installed");
    assert_eq!(state.codecs["paid-id"].failure_class, None);
    assert_eq!(state.codecs["paid-id"].last_attempt_utc, t0());
    assert_eq!(
        state.codecs["paid-id"].package,
        "Microsoft.HEVCVideoExtension"
    );
}

#[test]
fn appx_present_skips_winget() {
    let mut env = Env::new();
    env.appx = Box::new(|_| true);
    env.winget = Box::new(|_| WingetResult::exited(0, ""));
    let (_, seen) = run_codecs(&env, &mut CodecState::default());
    assert!(seen[0].installed);
    assert_eq!(seen[0].detail, "appx");
    assert!(lock(&env.winget_calls).is_empty());
}

#[test]
fn winget_success_only_counts_when_verified() {
    let mut env = Env::new();
    env.winget = Box::new(|_| WingetResult::exited(0, ""));
    let mut state = CodecState::default();
    let (_, seen) = run_codecs(&env, &mut state);
    assert!(!seen[0].installed);
    assert_eq!(seen[0].detail, codec::FAIL_UNVERIFIED_INSTALL);
    assert_eq!(state.codecs["paid-id"].status, "failed");
    assert_eq!(
        state.codecs["paid-id"].failure_class.as_deref(),
        Some(codec::FAIL_UNVERIFIED_INSTALL)
    );
    assert_eq!(*lock(&env.winget_calls), vec!["paid-id", "free-id"]);
}

#[test]
fn winget_success_verified_by_appx_probe_is_installed() {
    let installed = Arc::new(AtomicBool::new(false));
    let mut env = Env::new();
    let seen_installed = std::sync::Arc::clone(&installed);
    env.appx = Box::new(move |_| seen_installed.load(Ordering::SeqCst));
    let flip = installed;
    env.winget = Box::new(move |_| {
        flip.store(true, Ordering::SeqCst);
        WingetResult::exited(0, "")
    });
    let (_, seen) = run_codecs(&env, &mut CodecState::default());
    assert!(seen[0].installed);
    assert_eq!(seen[0].detail, "winget+appx");
}

#[test]
fn winget_success_verified_by_mf_reports_winget_mf() {
    let installed = Arc::new(AtomicBool::new(false));
    let mut env = Env::new();
    let seen_installed = std::sync::Arc::clone(&installed);
    env.mf = Box::new(move |_| seen_installed.load(Ordering::SeqCst));
    let flip = installed;
    env.winget = Box::new(move |_| {
        flip.store(true, Ordering::SeqCst);
        WingetResult::exited(0, "")
    });
    let (_, seen) = run_codecs(&env, &mut CodecState::default());
    assert_eq!(seen[0].detail, "winget+mf");
}

#[test]
fn winget_missing_stops_after_the_first_id() {
    let mut env = Env::new();
    env.winget = Box::new(|_| WingetResult::not_launched());
    let (_, seen) = run_codecs(&env, &mut CodecState::default());
    assert_eq!(lock(&env.winget_calls).len(), 1);
    assert_eq!(seen[0].detail, codec::FAIL_WINGET_MISSING);
}

#[test]
fn winget_failures_classify_by_output_tokens() {
    let cases = [
        (
            "No package found matching input criteria.",
            codec::FAIL_NOT_FOUND,
        ),
        ("This app requires PURCHASE.", codec::FAIL_NOT_ENTITLED),
        (
            "The app is not owned by this account",
            codec::FAIL_NOT_ENTITLED,
        ),
        ("Missing entitlement", codec::FAIL_NOT_ENTITLED),
        (
            "The msstore source is disabled by policy.",
            codec::FAIL_SOURCE_UNAVAILABLE,
        ),
        (
            "source agreement not accepted",
            codec::FAIL_SOURCE_UNAVAILABLE,
        ),
        ("Source blocked", codec::FAIL_SOURCE_UNAVAILABLE),
        ("disabled", codec::FAIL_UNKNOWN),
        ("something new and strange", codec::FAIL_UNKNOWN),
    ];
    for (output, expected) in cases {
        assert_eq!(
            codec::classify_winget_failure(&WingetResult::exited(1, output)),
            expected,
            "{output}"
        );
    }
    assert_eq!(
        codec::classify_winget_failure(&WingetResult::timed_out()),
        codec::FAIL_TIMEOUT
    );
    assert_eq!(
        codec::classify_winget_failure(&WingetResult::not_launched()),
        codec::FAIL_WINGET_MISSING
    );
}

#[test]
fn required_list_pins_the_hevc_fallback_order_and_matches_the_wire_list() {
    let hevc = codec::REQUIRED
        .iter()
        .find(|c| c.probe == "h265")
        .unwrap_or_else(|| panic!("no hevc"));
    assert_eq!(hevc.store_ids, ["9NMZLZ57R3T7", "9N4WGH0Z6VHQ"]);
    let mut probes: Vec<&str> = codec::REQUIRED.iter().map(|c| c.probe).collect();
    probes.sort_unstable();
    let mut wire_list = wire::EXTENSION_BACKED_VIDEO_CODECS.to_vec();
    wire_list.sort_unstable();
    assert_eq!(probes, wire_list);
}

#[test]
fn fourcc_matches_the_media_foundation_subtypes() {
    assert_eq!(codec::fourcc("h265"), Some(0x4356_4548));
    assert_eq!(codec::fourcc("vp9"), Some(0x3039_5056));
    assert_eq!(codec::fourcc("av1"), Some(0x3130_5641));
    assert_eq!(codec::fourcc("h264"), None);
    assert_eq!(codec::probe_decoder("h264"), None);
}

#[test]
fn report_messages_say_when_the_retry_happens() {
    let outcome = |installed, detail| CodecOutcome {
        codec: HEVC,
        installed,
        detail,
    };
    assert_eq!(
        codec::report_message(&outcome(true, "winget+mf")),
        (true, "HEVC Video Extensions ready (winget+mf).".to_owned())
    );
    assert_eq!(
        codec::report_message(&outcome(false, codec::FAIL_TIMEOUT)).1,
        "HEVC Video Extensions install failed (timeout); will retry tomorrow."
    );
    assert_eq!(
        codec::report_message(&outcome(false, codec::FAIL_NOT_ENTITLED)).1,
        "HEVC Video Extensions install failed (not_entitled); will retry next week."
    );
}

#[test]
fn capabilities_refresh_logs_and_stores_the_probe() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log_path = dir.path().join("w.log");
    let results = Arc::new(Mutex::new(VecDeque::from([
        None,
        Some(vec![]),
        Some(vec!["h265".to_owned(), "vp9".to_owned()]),
    ])));
    let feed = results;
    let caps = CodecCapabilities::new(
        Arc::new(move || lock(&feed).pop_front().flatten()),
        Log::to_file(&log_path),
    );
    assert_eq!(caps.verified(), None);
    caps.refresh();
    assert_eq!(caps.verified(), None);
    caps.refresh();
    assert_eq!(caps.verified(), Some(vec![]));
    caps.refresh();
    assert_eq!(
        caps.verified(),
        Some(vec!["h265".to_owned(), "vp9".to_owned()])
    );
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    for want in [
        "[codec] capability probe: unavailable",
        "[codec] capability probe: none",
        "[codec] capability probe: h265,vp9",
    ] {
        assert!(log.contains(want), "{want}: {log}");
    }
}

#[test]
fn disabled_codec_check_logs_and_spawns_nothing() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log_path = dir.path().join("w.log");
    let caps = Arc::new(CodecCapabilities::new(Arc::new(|| None), Log::disabled()));
    let (ux, _) = capture_ux();
    let handle =
        codec::start_background_check(false, dir.path(), caps, ux, Log::to_file(&log_path));
    assert!(handle.is_none());
    assert!(
        fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains("[codec] auto-install disabled by settings")
    );
}
