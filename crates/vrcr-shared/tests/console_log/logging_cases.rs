use super::*;

#[test]
fn wrapper_fallback_detail_is_file_only() {
    let (ux, buf) = ux(Style::plain());
    let dir = TempDir::new().expect("valid console fixture");
    let path = dir.path().join("detail.log");
    ux.tee_to(Log::to_file(&path));
    ux.wrapper_fallback("youtube.com", "pipe_down", 42);
    assert_eq!(buf.text(), "");
    let logged = fs::read_to_string(path).expect("valid console fixture");
    assert!(logged.contains(&format!(
        "[12:34:56] * [wrapper] !!  fallback  youtube.com{}pipe_down   elapsed=42ms",
        sp(24)
    )));
}

#[test]
fn utc_stamps_format_like_the_old_logger() {
    let at = UNIX_EPOCH + Duration::from_millis(1_760_400_000_123);
    let s = UtcStamp::at(at);
    assert_eq!(s.log_prefix(), "2025-10-14T00:00:00.123Z");
    assert_eq!(s.file_stamp(), "20251014-000000-123");
    assert_eq!(
        UtcStamp::at(UNIX_EPOCH).log_prefix(),
        "1970-01-01T00:00:00.000Z"
    );
}

#[test]
fn log_lines_carry_a_utc_prefix_and_track_the_last_write() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("x.log");
    let log = Log::to_file(&path);
    assert!(log.last_write().is_none());
    log.line("hello");
    assert!(log.last_write().is_some());
    log.close();
    log.line("dropped after close");
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    assert_eq!(first.len(), "2026-10-14T00:00:00.000Z hello".len());
    assert!(first.ends_with("Z hello"));
    assert!(lines.next().is_none());
}

#[test]
fn installed_log_names_its_file_by_component_and_prunes_old_ones() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let old = dir.path().join("watchdog-20200101-000000-000.log");
    let other = dir.path().join("updater-20200101-000000-000.log");
    for p in [&old, &other] {
        fs::write(p, b"x").unwrap_or_else(|e| panic!("{e}"));
        let f = fs::File::options()
            .write(true)
            .open(p)
            .unwrap_or_else(|e| panic!("{e}"));
        f.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 3600))
            .unwrap_or_else(|e| panic!("{e}"));
    }

    let log = Log::install(dir.path(), "watchdog");
    log.line("started");
    let current = log.current_path().unwrap_or_default();
    log.close();

    assert!(!old.exists());
    assert!(other.exists());
    let name = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert!(
        name.starts_with("watchdog-")
            && name.ends_with(".log")
            && name.len() == "watchdog-20261014-000000-000.log".len(),
        "{name}"
    );
    assert!(
        fs::read_to_string(&current)
            .unwrap_or_default()
            .ends_with("Z started\n")
    );
}

#[test]
fn prune_keeps_recent_logs_and_unrelated_files() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let recent = dir.path().join("watchdog-20261013-000000-000.log");
    let unrelated = dir.path().join("watchdog.txt");
    fs::write(&recent, b"x").unwrap_or_else(|e| panic!("{e}"));
    fs::write(&unrelated, b"x").unwrap_or_else(|e| panic!("{e}"));
    log::prune(
        dir.path(),
        "watchdog",
        SystemTime::now() + Duration::from_secs(30 * 24 * 3600),
    );
    assert!(!recent.exists());
    assert!(unrelated.exists());
}

#[test]
fn prune_drops_the_oldest_logs_past_the_size_budget() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let names = [
        "watchdog-20261010-000000-000.log",
        "watchdog-20261011-000000-000.log",
        "watchdog-20261012-000000-000.log",
        "watchdog-20261013-000000-000.log",
    ];
    for name in names {
        fs::write(dir.path().join(name), [b'x'; 100]).unwrap_or_else(|e| panic!("{e}"));
    }
    let other = dir.path().join("updater-20261001-000000-000.log");
    let crash = dir.path().join("crash-watchdog-20261001-000000.log");
    for p in [&other, &crash] {
        fs::write(p, [b'x'; 500]).unwrap_or_else(|e| panic!("{e}"));
    }

    log::prune_within(dir.path(), "watchdog", SystemTime::now(), 250);

    let left: Vec<bool> = names.iter().map(|n| dir.path().join(n).exists()).collect();
    assert_eq!(left, vec![false, false, true, true]);
    assert!(other.exists());
    assert!(crash.exists());
    assert_eq!(log::MAX_TOTAL_BYTES, 64 * 1024 * 1024);
    const { assert!(log::MAX_BYTES < log::MAX_TOTAL_BYTES) };
}

#[test]
fn disabled_log_swallows_lines() {
    let log = Log::disabled();
    log.line("nothing");
    assert!(!log.is_enabled());
    assert!(log.current_path().is_none());
}

#[test]
fn crash_report_redacts_the_profile_and_user_name() {
    let r = Report {
        component: "watchdog",
        kind: "panic",
        terminating: true,
        pid: 42,
        version: "2026.10.14.0",
        basedir: r"C:\Users\Alice\AppData\Local\vrcresolver",
        os: "windows",
        snapshot: Some("mesh=up user=ALICE".to_owned()),
        detail: r"panic at c:\users\alice\src\main.rs:1",
    };
    let text = crash::render(&r, UNIX_EPOCH);
    let red = crash::redact(&text, Some(r"C:\Users\Alice"), Some("alice"));
    assert!(red.starts_with(
        "=== vrcresolver crash log ===\ntimestamp:    1970-01-01T00:00:00.0000000Z\n"
    ));
    assert!(red.contains("terminating:  True\n"));
    assert!(red.contains(r"basedir:      %USERPROFILE%\AppData\Local\vrcresolver"));
    assert!(red.contains("--- state snapshot ---\nmesh=up user=<user>\n"));
    assert!(red.contains(r"panic at %USERPROFILE%\src\main.rs:1"));
    assert!(!red.to_ascii_lowercase().contains("alice"));
}

#[test]
fn crash_redaction_skips_short_user_names() {
    assert_eq!(crash::redact("al is here", None, Some("al")), "al is here");
    assert_eq!(crash::redact("x", Some(""), None), "x");
}
