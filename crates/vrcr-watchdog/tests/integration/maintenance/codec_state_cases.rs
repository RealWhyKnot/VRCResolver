use super::*;

#[test]
fn backoff_retries_transient_after_a_day_and_permanent_after_a_week() {
    let env = Env::new();
    let installer = Installer::new(&env);
    let hours = |h: u64| Duration::from_secs(h * 3600);
    let mut state = CodecState::default();
    let cases = [
        (Some(codec::FAIL_TIMEOUT), hours(12), true),
        (Some(codec::FAIL_TIMEOUT), hours(25), false),
        (Some(codec::FAIL_UNVERIFIED_INSTALL), hours(23), true),
        (None, hours(23), true),
        (None, hours(25), false),
        (Some(codec::FAIL_NOT_ENTITLED), hours(72), true),
        (Some(codec::FAIL_NOT_ENTITLED), hours(192), false),
        (Some(codec::FAIL_WINGET_MISSING), hours(160), true),
    ];
    for (class, ago, skip) in cases {
        state
            .codecs
            .insert("paid-id".to_owned(), failed(class, ago));
        assert_eq!(
            installer.should_skip(&state, "paid-id"),
            skip,
            "{class:?} {ago:?}"
        );
    }
    state.codecs.insert(
        "paid-id".to_owned(),
        CodecEntry {
            last_attempt_utc: t0() + hours(1),
            ..failed(Some(codec::FAIL_NOT_FOUND), Duration::ZERO)
        },
    );
    assert!(installer.should_skip(&state, "paid-id"));
    state.codecs.insert(
        "paid-id".to_owned(),
        CodecEntry {
            status: "weird".to_owned(),
            ..failed(None, Duration::ZERO)
        },
    );
    assert!(!installer.should_skip(&state, "paid-id"));
    assert!(!installer.should_skip(&state, "other"));
}

#[test]
fn installed_state_is_never_retried() {
    let mut env = Env::new();
    env.winget = Box::new(|_| WingetResult::exited(0, ""));
    let mut state = CodecState::default();
    state.codecs.insert(
        "paid-id".to_owned(),
        CodecEntry {
            status: "installed".to_owned(),
            ..failed(None, Duration::ZERO)
        },
    );
    let (dirty, seen) = run_codecs(&env, &mut state);
    assert!(!dirty);
    assert_eq!(
        seen,
        <[vrcr_watchdog::maintenance::codec::CodecOutcome; 0]>::default()
    );
    assert!(lock(&env.winget_calls).is_empty());
}

#[test]
fn legacy_state_file_without_failure_class_still_parses() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("codec-state.json");
    fs::write(
        &path,
        r#"{"codecs":{"9NMZLZ57R3T7":{"status":"failed","last_attempt_utc":"2026-08-20T00:00:00Z","package_family_name":"Microsoft.HEVCVideoExtension"}}}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let state = codec::load_state(&path);
    let entry = &state.codecs["9NMZLZ57R3T7"];
    assert_eq!(entry.status, "failed");
    assert_eq!(entry.failure_class, None);
    assert_eq!(entry.package, "Microsoft.HEVCVideoExtension");
    assert_eq!(
        entry.last_attempt_utc,
        UNIX_EPOCH + Duration::from_secs(crate::common::T_2026_08_21 - 36 * 3600)
    );
}

#[test]
fn csharp_written_state_with_fractional_time_and_null_class_parses() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("codec-state.json");
    fs::write(
        &path,
        r#"{"codecs":{"a":{"status":"installed","failure_class":null,"last_attempt_utc":"2026-08-21T12:00:00.1234567Z","package_family_name":"p"},"b":{"status":"failed","failure_class":"timeout","last_attempt_utc":null,"package_family_name":"q","extra":1}}}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let state = codec::load_state(&path);
    assert_eq!(state.codecs["a"].status, "installed");
    assert_eq!(
        state.codecs["a"].last_attempt_utc,
        t0() + Duration::from_nanos(123_456_700)
    );
    assert_eq!(state.codecs["b"].last_attempt_utc, UNIX_EPOCH);
    assert_eq!(state.codecs["b"].failure_class.as_deref(), Some("timeout"));
}

#[test]
fn state_round_trips_and_garbage_loads_empty() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("codec-state.json");
    let mut state = CodecState::default();
    state.codecs.insert(
        "x".to_owned(),
        failed(Some("timeout"), Duration::from_secs(5)),
    );
    codec::save_state(&path, &state);
    assert_eq!(codec::load_state(&path), state);
    assert!(!dir.path().join("codec-state.json.new").exists());
    let text = fs::read_to_string(&path).unwrap_or_default();
    assert!(text.contains("\"package_family_name\""), "{text}");
    assert!(
        text.contains("\"last_attempt_utc\":\"2026-08-21T11:59:55"),
        "{text}"
    );

    fs::write(&path, "{not json").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(codec::load_state(&path), CodecState::default());
    assert_eq!(
        codec::load_state(&dir.path().join("missing.json")),
        CodecState::default()
    );
}

#[test]
fn run_check_reports_saves_state_and_refreshes_capabilities() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("codec-state.json");
    let refreshes = Arc::new(AtomicUsize::new(0));
    let count = std::sync::Arc::clone(&refreshes);
    let caps = CodecCapabilities::new(
        Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Some(vec!["av1".to_owned()])
        }),
        Log::disabled(),
    );
    let mut env = Env::new();
    env.mf = Box::new(|c| c == "av1");
    env.winget = Box::new(|_| WingetResult::exited(1, "No package found"));
    let (ux, out) = capture_ux();
    block_on(codec::run_check(&Installer::new(&env), &path, &caps, &ux));
    assert_eq!(
        out.text(),
        "  AV1 video support is ready.\n  HEVC video support couldn't be installed. VRCResolver will try again next week.\n  VP9 video support couldn't be installed. VRCResolver will try again next week.\n"
    );
    let text = out.logs();
    assert!(text.contains("AV1 Video Extension ready (mf)."), "{text}");
    assert!(
        text.contains("HEVC Video Extensions install failed (not_found); will retry next week."),
        "{text}"
    );
    assert!(
        text.contains("VP9 Video Extensions install failed (not_found)"),
        "{text}"
    );
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    let saved = codec::load_state(&path);
    assert_eq!(saved.codecs.len(), 3);
    assert_eq!(saved.codecs["9MVZQVXJBQ9V"].status, "installed");
    assert_eq!(saved.codecs["9NMZLZ57R3T7"].status, "failed");

    let before = fs::read(&path).unwrap_or_default();
    block_on(codec::run_check(&Installer::new(&env), &path, &caps, &ux));
    assert_eq!(fs::read(&path).unwrap_or_default(), before);
}
