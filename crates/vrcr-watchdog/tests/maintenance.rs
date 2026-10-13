mod common;

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::mesh::Capture;
use serde_json::json;
use tempfile::TempDir;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::wire;
use vrcr_watchdog::maintenance::codec::{
    self, Codec, CodecCapabilities, CodecEntry, CodecEnv, CodecOutcome, CodecState, Installer, WingetResult,
};
use vrcr_watchdog::maintenance::shutdown::{ShutdownCoordinator, ShutdownStep};
use vrcr_watchdog::maintenance::update::{self, Release, UpdateStart, Version};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (Ux::new(Box::new(out.clone()), Style::plain(), String::new), out)
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| panic!("{e}"))
        .block_on(f)
}

const HEVC: Codec = Codec {
    name: "HEVC Video Extensions",
    package: "Microsoft.HEVCVideoExtension",
    probe: "h265",
    store_ids: &["paid-id", "free-id"],
};

fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(common::T_2026_08_21)
}

type Winget = Box<dyn Fn(&str) -> WingetResult + Send + Sync>;

struct Env {
    mf: Box<dyn Fn(&str) -> bool + Send + Sync>,
    appx: Box<dyn Fn(&str) -> bool + Send + Sync>,
    winget: Winget,
    now: SystemTime,
    appx_calls: AtomicUsize,
    winget_calls: Mutex<Vec<String>>,
}

impl Env {
    fn new() -> Env {
        Env {
            mf: Box::new(|_| false),
            appx: Box::new(|_| false),
            winget: Box::new(|_| WingetResult::exited(1, "")),
            now: t0(),
            appx_calls: AtomicUsize::new(0),
            winget_calls: Mutex::new(Vec::new()),
        }
    }
}

impl CodecEnv for &Env {
    fn mf_probe(&self, codec: &str) -> bool {
        (self.mf)(codec)
    }

    async fn appx_probe(&self, package: &str) -> bool {
        self.appx_calls.fetch_add(1, Ordering::SeqCst);
        (self.appx)(package)
    }

    async fn winget_install(&self, store_id: &str) -> WingetResult {
        lock(&self.winget_calls).push(store_id.to_owned());
        (self.winget)(store_id)
    }

    fn now(&self) -> SystemTime {
        self.now
    }
}

fn run_codecs(env: &Env, state: &mut CodecState) -> (bool, Vec<CodecOutcome>) {
    let installer = Installer::new(env);
    let mut seen = Vec::new();
    let dirty = block_on(installer.run(&[HEVC], state, |o| seen.push(o.clone())));
    (dirty, seen)
}

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
    assert_eq!(state.codecs["paid-id"].package, "Microsoft.HEVCVideoExtension");
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
    let seen_installed = installed.clone();
    env.appx = Box::new(move |_| seen_installed.load(Ordering::SeqCst));
    let flip = installed.clone();
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
    let seen_installed = installed.clone();
    env.mf = Box::new(move |_| seen_installed.load(Ordering::SeqCst));
    let flip = installed.clone();
    env.winget = Box::new(move |_| {
        flip.store(true, Ordering::SeqCst);
        WingetResult::exited(0, "")
    });
    let (_, seen) = run_codecs(&env, &mut CodecState::default());
    assert_eq!(seen[0].detail, "winget+mf");
}

#[test]
fn second_store_id_is_tried_only_after_the_first_fails() {
    let installed = Arc::new(AtomicBool::new(false));
    let mut env = Env::new();
    let seen_installed = installed.clone();
    env.appx = Box::new(move |_| seen_installed.load(Ordering::SeqCst));
    let flip = installed.clone();
    env.winget = Box::new(move |id| {
        if id == "free-id" {
            flip.store(true, Ordering::SeqCst);
            WingetResult::exited(0, "")
        } else {
            WingetResult::exited(1, "This app requires purchase before install.")
        }
    });
    let (_, seen) = run_codecs(&env, &mut CodecState::default());
    assert_eq!(*lock(&env.winget_calls), vec!["paid-id", "free-id"]);
    assert!(seen[0].installed);
    assert_eq!(seen[0].detail, "winget+appx");
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
        ("No package found matching input criteria.", codec::FAIL_NOT_FOUND),
        ("This app requires PURCHASE.", codec::FAIL_NOT_ENTITLED),
        ("The app is not owned by this account", codec::FAIL_NOT_ENTITLED),
        ("Missing entitlement", codec::FAIL_NOT_ENTITLED),
        (
            "The msstore source is disabled by policy.",
            codec::FAIL_SOURCE_UNAVAILABLE,
        ),
        ("source agreement not accepted", codec::FAIL_SOURCE_UNAVAILABLE),
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

fn failed(class: Option<&str>, ago: Duration) -> CodecEntry {
    CodecEntry {
        status: "failed".to_owned(),
        failure_class: class.map(str::to_owned),
        last_attempt_utc: t0() - ago,
        package: String::new(),
    }
}

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
        state.codecs.insert("paid-id".to_owned(), failed(class, ago));
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
    assert!(seen.is_empty());
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
        UNIX_EPOCH + Duration::from_secs(common::T_2026_08_21 - 36 * 3600)
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
    state
        .codecs
        .insert("x".to_owned(), failed(Some("timeout"), Duration::from_secs(5)));
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
    let feed = results.clone();
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
    assert_eq!(caps.verified(), Some(vec!["h265".to_owned(), "vp9".to_owned()]));
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
fn run_check_reports_saves_state_and_refreshes_capabilities() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("codec-state.json");
    let refreshes = Arc::new(AtomicUsize::new(0));
    let count = refreshes.clone();
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
    let text = out.text();
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

#[test]
fn disabled_codec_check_logs_and_spawns_nothing() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log_path = dir.path().join("w.log");
    let caps = Arc::new(CodecCapabilities::new(Arc::new(|| None), Log::disabled()));
    let (ux, _) = capture_ux();
    let handle = codec::start_background_check(false, dir.path(), caps, ux, Log::to_file(&log_path));
    assert!(handle.is_none());
    assert!(
        fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains("[codec] auto-install disabled by settings")
    );
}

#[cfg(windows)]
#[test]
fn media_foundation_probe_answers_on_this_machine() {
    let all = codec::probe_all();
    let verified = all.unwrap_or_else(|| panic!("Media Foundation did not start"));
    for c in &verified {
        assert!(wire::EXTENSION_BACKED_VIDEO_CODECS.contains(&c.as_str()));
        assert_eq!(codec::probe_decoder(c), Some(true));
    }
}

#[test]
fn versions_parse_like_dotnet() {
    assert!(Version::parse("1").is_none());
    assert!(Version::parse("1.2.3.4.5").is_none());
    assert!(Version::parse("1.x").is_none());
    assert!(Version::parse("1.-2").is_none());
    assert!(Version::parse("1..2").is_none());
    assert!(Version::parse("1.2147483648").is_none());
    assert!(Version::parse("1.2") < Version::parse("1.2.0"));
    assert!(Version::parse("1.2.0") < Version::parse("1.2.0.0"));
    assert!(Version::parse("2026.5.10.5") < Version::parse("2026.6.0.0"));
    assert!(Version::parse("2026.10.1.0") > Version::parse("2026.9.30.9"));
    assert_eq!(
        Version::from_tag("vV2026.5.10.0-AAAA"),
        Version::parse("2026.5.10.0")
    );
    assert_eq!(Version::from_tag("garbage-tag"), None);
}

fn pick(doc: serde_json::Value) -> Option<Release> {
    update::pick_highest(&doc)
}

#[test]
fn pick_highest_picks_the_highest_version_not_the_newest() {
    let r = pick(json!([
        {"tag_name": "v2026.6.0.0-beta", "html_url": "https://example.test/beta", "prerelease": true},
        {"tag_name": "v2026.5.10.5", "html_url": "https://example.test/p5", "prerelease": false}
    ]))
    .unwrap_or_else(|| panic!("none"));
    assert_eq!(r.tag, "v2026.6.0.0-beta");
    assert!(r.prerelease);
}

#[test]
fn pick_highest_prefers_stable_on_a_tie_in_either_order() {
    for list in [
        json!([
            {"tag_name": "v2026.5.10.0-pre1", "html_url": "a", "prerelease": true},
            {"tag_name": "v2026.5.10.0", "html_url": "b", "prerelease": false}
        ]),
        json!([
            {"tag_name": "v2026.5.10.0", "html_url": "b", "prerelease": false},
            {"tag_name": "v2026.5.10.0-pre1", "html_url": "a", "prerelease": true}
        ]),
    ] {
        let r = pick(list).unwrap_or_else(|| panic!("none"));
        assert_eq!(r.tag, "v2026.5.10.0");
        assert!(!r.prerelease);
    }
}

#[test]
fn pick_highest_keeps_the_first_of_equal_releases() {
    let r = pick(json!([
        {"tag_name": "v2026.5.10.0", "html_url": "first", "prerelease": false},
        {"tag_name": "V2026.5.10.0", "html_url": "second", "prerelease": false},
        {"tag_name": "v2026.5.10.0-rc", "html_url": "third", "prerelease": true}
    ]))
    .unwrap_or_else(|| panic!("none"));
    assert_eq!(r.html_url, "first");
}

#[test]
fn pick_highest_skips_junk_and_handles_empty() {
    assert_eq!(pick(json!([])), None);
    assert_eq!(pick(json!({"tag_name": "v1.0"})), None);
    let r = pick(json!([
        {"tag_name": "garbage-tag", "html_url": "x", "prerelease": false},
        "not an object",
        {"html_url": "no tag"},
        {"tag_name": "v2026.5.10.0", "html_url": "y", "prerelease": false}
    ]))
    .unwrap_or_else(|| panic!("none"));
    assert_eq!(r.tag, "v2026.5.10.0");
    let r = pick(json!([
        {"tag_name": "v2026.5.10.0-AAAA", "html_url": "x", "prerelease": true},
        {"tag_name": "v2026.5.9.5", "html_url": "y", "prerelease": false}
    ]))
    .unwrap_or_else(|| panic!("none"));
    assert_eq!(r.tag, "v2026.5.10.0-AAAA");
}

#[test]
fn available_message_only_for_a_newer_remote() {
    let stable =
        json!({"tag_name": "v2026.10.14.0", "html_url": "https://example.test/r", "prerelease": false});
    assert_eq!(
        update::available_message(&stable, false, "2026.10.13.2").as_deref(),
        Some("version v2026.10.14.0 is available; type /update to install (https://example.test/r)")
    );
    assert_eq!(update::available_message(&stable, false, "2026.10.14.0"), None);
    assert_eq!(update::available_message(&stable, false, "2026.10.15.0"), None);
    assert_eq!(update::available_message(&stable, false, "not-a-version"), None);
    let bare = json!({"tag_name": "v9.0", "prerelease": true});
    assert_eq!(
        update::available_message(&bare, false, "1.0").as_deref(),
        Some("version v9.0 (prerelease) is available; type /update to install")
    );
    let list = json!([{"tag_name": "v9.0-rc", "html_url": "u", "prerelease": true}]);
    assert_eq!(
        update::available_message(&list, true, "1.0").as_deref(),
        Some("version v9.0-rc (prerelease) is available; type /update to install (u)")
    );
    assert_eq!(update::available_message(&list, false, "1.0"), None);
}

#[test]
fn check_asks_the_channel_endpoint() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    for pre in [false, true] {
        let seen = asked.clone();
        let msg = block_on(update::check(pre, "1.0", move |url| {
            lock(&seen).push(url);
            async { Some(r#"[{"tag_name":"v2.0"}]"#.to_owned()) }
        }));
        assert_eq!(msg.is_some(), pre);
    }
    assert_eq!(
        *lock(&asked),
        vec![update::STABLE_LATEST_URL, update::ANY_RELEASES_URL]
    );
    assert_eq!(block_on(update::check(false, "1.0", |_| async { None })), None);
    assert_eq!(
        block_on(update::check(false, "1.0", |_| async {
            Some("<html>".to_owned())
        })),
        None
    );
}

#[test]
fn disabled_update_check_logs_and_spawns_nothing() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log_path = dir.path().join("w.log");
    let (ux, _) = capture_ux();
    assert!(update::start_background_check(false, false, "1.0", ux, &Log::to_file(&log_path)).is_none());
    assert!(
        fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains("[update] startup update check disabled by settings")
    );
}

#[test]
fn updater_command_carries_the_request_env() {
    let install = std::env::temp_dir().join("vrcresolver");
    let cmd = update::updater_command(&install);
    assert_eq!(
        Path::new(cmd.get_program()),
        install.join("vrcresolver.Updater.exe")
    );
    assert_eq!(cmd.get_current_dir(), Some(install.as_path()));
    let env: Vec<_> = cmd.get_envs().collect();
    assert_eq!(
        env,
        vec![(
            std::ffi::OsStr::new(update::UPDATE_REQUESTED_ENV),
            Some(std::ffi::OsStr::new("1"))
        )]
    );
}

#[test]
fn start_update_reports_missing_failed_and_started() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let missing = update::start_update(dir.path(), |_| panic!("must not spawn"));
    assert_eq!(
        missing,
        UpdateStart::UpdaterMissing(dir.path().join(update::UPDATER_EXE))
    );
    assert!(
        missing
            .message()
            .starts_with("updater not found in install folder: ")
    );
    fs::write(dir.path().join(update::UPDATER_EXE), b"x").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        update::start_update(dir.path(), |_| Err(std::io::Error::other("nope"))),
        UpdateStart::Failed
    );
    let spawned = Arc::new(Mutex::new(None));
    let slot = spawned.clone();
    assert_eq!(
        update::start_update(dir.path(), move |cmd| {
            *lock(&slot) = Some(cmd.get_program().to_owned());
            Ok(())
        }),
        UpdateStart::Started
    );
    assert_eq!(
        lock(&spawned).clone().map(std::path::PathBuf::from),
        Some(dir.path().join(update::UPDATER_EXE))
    );
    assert_eq!(UpdateStart::Failed.message(), "update could not start.");
}

#[test]
fn staged_updater_replaces_the_stale_one() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    fs::write(root.join("vrcresolver.Updater.exe"), "OLD").unwrap_or_else(|e| panic!("{e}"));
    fs::write(root.join("vrcresolver.Updater.next.exe"), "NEW").unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    assert!(update::apply_staged_updater(root, &ux, &Log::disabled()));
    assert_eq!(
        fs::read_to_string(root.join("vrcresolver.Updater.exe")).unwrap_or_default(),
        "NEW"
    );
    assert!(!root.join("vrcresolver.Updater.next.exe").exists());
    let names: Vec<String> = fs::read_dir(root)
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["vrcresolver.Updater.exe".to_owned()]);
    assert!(out.text().contains("updater refreshed for future updates."));
}

#[test]
fn staged_updater_installs_when_no_updater_exists_yet() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("vrcresolver.Updater.next.exe"), "NEW").unwrap_or_else(|e| panic!("{e}"));
    let (ux, _) = capture_ux();
    assert!(update::apply_staged_updater(dir.path(), &ux, &Log::disabled()));
    assert_eq!(
        fs::read_to_string(dir.path().join("vrcresolver.Updater.exe")).unwrap_or_default(),
        "NEW"
    );
}

#[test]
fn staged_updater_ignores_pre_rename_names_and_noops_without_a_stage() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    fs::write(root.join("WKVRCProxy.Updater.exe"), "OLD-REAL-UPDATER").unwrap_or_else(|e| panic!("{e}"));
    fs::write(root.join("WKVRCProxy.Updater.next.exe"), "LAUNCHER").unwrap_or_else(|e| panic!("{e}"));
    fs::write(root.join("vrcresolver.Updater.exe"), "OLD").unwrap_or_else(|e| panic!("{e}"));
    let (ux, _) = capture_ux();
    assert!(!update::apply_staged_updater(root, &ux, &Log::disabled()));
    assert_eq!(
        fs::read_to_string(root.join("WKVRCProxy.Updater.exe")).unwrap_or_default(),
        "OLD-REAL-UPDATER"
    );
    assert_eq!(
        fs::read_to_string(root.join("vrcresolver.Updater.exe")).unwrap_or_default(),
        "OLD"
    );
}

#[cfg(windows)]
#[test]
fn staged_updater_keeps_the_old_one_when_the_stage_cannot_move_in() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    fs::write(root.join("vrcresolver.Updater.exe"), "OLD").unwrap_or_else(|e| panic!("{e}"));
    fs::write(root.join("vrcresolver.Updater.next.exe"), "NEW").unwrap_or_else(|e| panic!("{e}"));
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(root.join("vrcresolver.Updater.next.exe"))
        .unwrap_or_else(|e| panic!("{e}"));
    let log_path = root.join("w.log");
    let (ux, out) = capture_ux();
    let applied = update::apply_staged_updater(root, &ux, &Log::to_file(&log_path));
    drop(held);
    assert!(!applied);
    assert_eq!(
        fs::read_to_string(root.join("vrcresolver.Updater.exe")).unwrap_or_default(),
        "OLD"
    );
    assert!(root.join("vrcresolver.Updater.next.exe").is_file());
    assert!(
        out.text()
            .contains("could not refresh updater yet; it will retry on next launch.")
    );
    assert!(
        fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains("[update] staged updater repair failed: ")
    );
    let leftovers = fs::read_dir(root)
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".old-"))
        .count();
    assert_eq!(leftovers, 0);
}

struct ShutdownRig {
    log: Arc<Mutex<Vec<String>>>,
}

impl ShutdownRig {
    fn new() -> ShutdownRig {
        ShutdownRig {
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn coordinator(
        &self,
        restore: impl Fn(&str) -> Result<bool, String> + Send + Sync + 'static,
        steps: impl Fn() -> Vec<ShutdownStep> + Send + Sync + 'static,
    ) -> ShutdownCoordinator {
        let log = self.log.clone();
        ShutdownCoordinator::new(
            Arc::new(restore),
            Arc::new(steps),
            Arc::new(move |line: &str| lock(&log).push(line.to_owned())),
        )
    }

    fn lines(&self) -> Vec<String> {
        lock(&self.log).clone()
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap_or_else(|e| panic!("{e}"))
}

const GENEROUS: Duration = Duration::from_secs(5);

#[test]
fn second_caller_shares_the_run_and_waits_for_the_restore() {
    let rig = ShutdownRig::new();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let wait = gate.clone();
    let c = rig.coordinator(
        move |_| {
            let (m, cv) = &*wait;
            let mut open = lock(m);
            while !*open {
                open = cv.wait(open).unwrap_or_else(|e| e.into_inner());
            }
            Ok(true)
        },
        Vec::new,
    );
    rt().block_on(async {
        let first = c.run("ctrl-c", false);
        let second = c.run("quit", false);
        let second = tokio::spawn(second);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!second.is_finished());
        {
            let (m, cv) = &*gate;
            *lock(m) = true;
            cv.notify_all();
        }
        tokio::time::timeout(GENEROUS, second)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        tokio::time::timeout(GENEROUS, first)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    let lines = rig.lines();
    assert!(lines.contains(&"shutdown trigger=ctrl-c".to_owned()));
    assert!(!lines.iter().any(|l| l.contains("quit")));
}

#[test]
fn restore_runs_before_slow_steps() {
    let rig = ShutdownRig::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let started = Instant::now();
    let restored_at = Arc::new(Mutex::new(None));
    let (o1, o2, at) = (order.clone(), order.clone(), restored_at.clone());
    let c = rig.coordinator(
        move |_| {
            *lock(&at) = Some(started.elapsed());
            lock(&o1).push("restore");
            Ok(true)
        },
        move || {
            let o = o2.clone();
            vec![ShutdownStep::new(
                "slow",
                Duration::from_secs(10),
                move || async move {
                    lock(&o).push("slow");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok(())
                },
            )]
        },
    );
    rt().block_on(async {
        let run = c.run("quit", false);
        drop(tokio::spawn(run));
        let deadline = Instant::now() + GENEROUS;
        while lock(&restored_at).is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let at = lock(&restored_at).unwrap_or_else(|| panic!("restore never ran"));
    assert!(at < Duration::from_secs(1), "{at:?}");
    assert_eq!(lock(&order).first().copied(), Some("restore"));
}

#[test]
fn fast_shutdown_restores_and_skips_steps() {
    let rig = ShutdownRig::new();
    let restored = Arc::new(AtomicBool::new(false));
    let ran = Arc::new(AtomicBool::new(false));
    let (r, s) = (restored.clone(), ran.clone());
    let c = rig.coordinator(
        move |_| {
            r.store(true, Ordering::SeqCst);
            Ok(true)
        },
        move || {
            let s = s.clone();
            vec![ShutdownStep::new(
                "terminal",
                Duration::from_secs(1),
                move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )]
        },
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("console-close", true))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(restored.load(Ordering::SeqCst));
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(
        rig.lines(),
        vec!["shutdown trigger=console-close", "tools restored=true"]
    );
}

#[test]
fn restore_failure_is_logged_and_steps_still_run() {
    for restore_errors in [false, true] {
        let rig = ShutdownRig::new();
        let ran = Arc::new(AtomicBool::new(false));
        let s = ran.clone();
        let c = rig.coordinator(
            move |_| {
                if restore_errors {
                    Err("PermissionDenied: tools locked".to_owned())
                } else {
                    Ok(false)
                }
            },
            move || {
                let s = s.clone();
                vec![ShutdownStep::new(
                    "ipc",
                    Duration::from_secs(1),
                    move || async move {
                        s.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )]
            },
        );
        rt().block_on(async {
            tokio::time::timeout(GENEROUS, c.run("quit", false))
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        });
        assert!(ran.load(Ordering::SeqCst));
        let lines = rig.lines();
        assert!(lines.contains(&"tools restored=false".to_owned()));
        assert_eq!(
            lines
                .iter()
                .any(|l| l.contains("tools restore threw") && l.contains("tools locked")),
            restore_errors
        );
    }
}

#[test]
fn restore_panic_is_logged_and_steps_still_run() {
    let rig = ShutdownRig::new();
    let ran = Arc::new(AtomicBool::new(false));
    let s = ran.clone();
    let c = rig.coordinator(
        |_| panic!("boom"),
        move || {
            let s = s.clone();
            vec![ShutdownStep::new(
                "ipc",
                Duration::from_secs(1),
                move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )]
        },
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(ran.load(Ordering::SeqCst));
    assert!(rig.lines().contains(&"tools restore threw panicked".to_owned()));
}

#[test]
fn step_over_budget_does_not_block_later_steps() {
    let rig = ShutdownRig::new();
    let later = Arc::new(AtomicBool::new(false));
    let s = later.clone();
    let c = rig.coordinator(
        |_| Ok(true),
        move || {
            let s = s.clone();
            vec![
                ShutdownStep::new("mesh", Duration::from_millis(100), std::future::pending),
                ShutdownStep::new("resolve-cache", Duration::from_secs(1), move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                }),
            ]
        },
    );
    let clock = Instant::now();
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(later.load(Ordering::SeqCst));
    assert!(clock.elapsed() < Duration::from_secs(2));
    assert!(
        rig.lines()
            .contains(&"shutdown step mesh took longer than 100 ms; moving on".to_owned()),
        "{:?}",
        rig.lines()
    );
}

#[test]
fn failing_or_panicking_steps_do_not_block_later_steps() {
    let rig = ShutdownRig::new();
    let later = Arc::new(AtomicBool::new(false));
    let s = later.clone();
    let c = rig.coordinator(
        |_| Ok(true),
        move || {
            let s = s.clone();
            vec![
                ShutdownStep::new("relay", Duration::from_secs(1), || async {
                    Err("listener gone".to_owned())
                }),
                ShutdownStep::new("heartbeat", Duration::from_secs(1), || async { panic!("bad") }),
                ShutdownStep::new("mesh", Duration::from_secs(1), move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                }),
            ]
        },
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(later.load(Ordering::SeqCst));
    let lines = rig.lines();
    assert!(lines.contains(&"shutdown step relay failed: listener gone".to_owned()));
    assert!(lines.contains(&"shutdown step heartbeat failed: panicked".to_owned()));
}

#[test]
fn spent_budget_skips_the_remaining_steps() {
    let rig = ShutdownRig::new();
    let ran = Arc::new(AtomicBool::new(false));
    let s = ran.clone();
    let c = rig
        .coordinator(
            |_| {
                std::thread::sleep(Duration::from_millis(80));
                Ok(true)
            },
            move || {
                let s = s.clone();
                vec![ShutdownStep::new(
                    "ipc",
                    Duration::from_secs(1),
                    move || async move {
                        s.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )]
            },
        )
        .with_total_budget(Duration::from_millis(50));
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(!ran.load(Ordering::SeqCst));
    assert!(
        rig.lines()
            .contains(&"shutdown budget spent; skipping ipc and the rest".to_owned())
    );
}

#[test]
fn shutdown_before_steps_exist_still_restores_with_the_trigger() {
    let rig = ShutdownRig::new();
    let reason = Arc::new(Mutex::new(String::new()));
    let r = reason.clone();
    let c = rig.coordinator(
        move |why| {
            *lock(&r) = why.to_owned();
            Ok(true)
        },
        Vec::new,
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("ctrl-c", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert_eq!(*lock(&reason), "ctrl-c");
    assert_eq!(
        rig.lines(),
        vec!["shutdown trigger=ctrl-c", "tools restored=true"]
    );
}

#[test]
fn steps_are_read_when_the_shutdown_starts() {
    let rig = ShutdownRig::new();
    let registered = Arc::new(AtomicBool::new(false));
    let ran = Arc::new(AtomicBool::new(false));
    let (reg, s) = (registered.clone(), ran.clone());
    let c = rig.coordinator(
        |_| Ok(true),
        move || {
            if !reg.load(Ordering::SeqCst) {
                return Vec::new();
            }
            let s = s.clone();
            vec![ShutdownStep::new(
                "heartbeat",
                Duration::from_secs(1),
                move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )]
        },
    );
    registered.store(true, Ordering::SeqCst);
    assert!(!c.started());
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(c.started());
    assert!(ran.load(Ordering::SeqCst));
}

#[cfg(windows)]
#[test]
fn parent_process_names_whoever_started_the_test_binary() {
    let parent = vrcr_watchdog::maintenance::describe_parent_process().unwrap_or_else(|| panic!("no parent"));
    let (name, pid) = parent.split_once(" pid=").unwrap_or_else(|| panic!("{parent}"));
    assert!(
        !name.is_empty() && !name.contains(' ') && !name.to_lowercase().ends_with(".exe"),
        "{parent}"
    );
    let pid: u32 = pid.parse().unwrap_or_else(|e| panic!("{parent}: {e}"));
    assert_ne!(pid, std::process::id());
    let procs = vrcr_shared::win::list_processes();
    let found = procs
        .iter()
        .find(|p| p.pid == pid)
        .unwrap_or_else(|| panic!("{parent}"));
    assert_eq!(vrcr_shared::win::process_stem(&found.exe), name);
    let mine = vrcr_shared::win::process_start_time(std::process::id());
    let theirs = vrcr_shared::win::process_start_time(pid);
    if let (Some(mine), Some(theirs)) = (mine, theirs) {
        assert!(theirs <= mine);
    }
}

#[test]
fn describe_parent_handles_missing_and_self_parented_entries() {
    use vrcr_shared::win::{ProcessInfo, describe_parent};
    let p = |pid, parent_pid, exe: &str| ProcessInfo {
        pid,
        parent_pid,
        exe: exe.to_owned(),
    };
    let list = vec![
        p(4, 0, "System"),
        p(100, 50, "cargo.exe"),
        p(200, 100, "tests.EXE"),
        p(300, 300, "odd.exe"),
    ];
    assert_eq!(describe_parent(&list, 200).as_deref(), Some("cargo pid=100"));
    assert_eq!(describe_parent(&list, 100), None);
    assert_eq!(describe_parent(&list, 300), None);
    assert_eq!(describe_parent(&list, 999), None);
    assert_eq!(vrcr_shared::win::process_stem("a.exe.exe"), "a.exe");
    assert_eq!(vrcr_shared::win::process_stem(".exe"), ".exe");
    assert_eq!(vrcr_shared::win::process_stem("VRChat"), "VRChat");
}
