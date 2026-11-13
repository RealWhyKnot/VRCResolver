use super::*;

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

#[test]
fn pick_highest_picks_the_highest_version_not_the_newest() {
    let r = pick(&json!([
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
        let r = pick(&list).unwrap_or_else(|| panic!("none"));
        assert_eq!(r.tag, "v2026.5.10.0");
        assert!(!r.prerelease);
    }
}

#[test]
fn pick_highest_keeps_the_first_of_equal_releases() {
    let r = pick(&json!([
        {"tag_name": "v2026.5.10.0", "html_url": "first", "prerelease": false},
        {"tag_name": "V2026.5.10.0", "html_url": "second", "prerelease": false},
        {"tag_name": "v2026.5.10.0-rc", "html_url": "third", "prerelease": true}
    ]))
    .unwrap_or_else(|| panic!("none"));
    assert_eq!(r.html_url, "first");
}

#[test]
fn pick_highest_skips_junk_and_handles_empty() {
    assert_eq!(pick(&json!([])), None);
    assert_eq!(pick(&json!({"tag_name": "v1.0"})), None);
    let r = pick(&json!([
        {"tag_name": "garbage-tag", "html_url": "x", "prerelease": false},
        "not an object",
        {"html_url": "no tag"},
        {"tag_name": "v2026.5.10.0", "html_url": "y", "prerelease": false}
    ]))
    .unwrap_or_else(|| panic!("none"));
    assert_eq!(r.tag, "v2026.5.10.0");
    let r = pick(&json!([
        {"tag_name": "v2026.5.10.0-AAAA", "html_url": "x", "prerelease": true},
        {"tag_name": "v2026.5.9.5", "html_url": "y", "prerelease": false}
    ]))
    .unwrap_or_else(|| panic!("none"));
    assert_eq!(r.tag, "v2026.5.10.0-AAAA");
}

#[test]
fn available_message_only_for_a_newer_remote() {
    let stable = json!({"tag_name": "v2026.10.14.0", "html_url": "https://example.test/r", "prerelease": false});
    assert_eq!(
        update::available_message(&stable, false, "2026.10.13.2").as_deref(),
        Some(
            "version v2026.10.14.0 is available; type /update to install (https://example.test/r)"
        )
    );
    assert_eq!(
        update::available_message(&stable, false, "2026.10.14.0"),
        None
    );
    assert_eq!(
        update::available_message(&stable, false, "2026.10.15.0"),
        None
    );
    assert_eq!(
        update::available_message(&stable, false, "not-a-version"),
        None
    );
    assert!(update::available_message(&stable, false, "2026.10.13.2-C0A0").is_some());
    assert_eq!(
        update::available_message(&stable, false, "2026.10.14.0-C0A0"),
        None
    );
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
        let seen = std::sync::Arc::clone(&asked);
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
    assert_eq!(
        block_on(update::check(false, "1.0", |_| async { None })),
        None
    );
    assert_eq!(
        block_on(update::check(false, "1.0", |_| async {
            Some("<html>".to_owned())
        })),
        None
    );
}

#[test]
fn update_announcement_uses_the_release_tag_and_keeps_the_log_detail() {
    let release = block_on(update::check(true, "1.0", |_| async {
        Some(r#"[{"tag_name":"v2.0-rc","html_url":"https://example.test/releases/v2.0-rc","prerelease":true}]"#.to_owned())
    })).expect("newer release");
    assert_eq!(release.tag, "v2.0-rc");
    let (ux, out) = capture_ux();
    release.announce(&ux);
    assert_eq!(
        out.text(),
        "  Version v2.0-rc is available. Type update to install it.\n"
    );
    assert!(out.logs().contains(
        "[update] version v2.0-rc (prerelease) is available; type /update to install (example.test"
    ));
}

#[test]
fn disabled_update_check_logs_and_spawns_nothing() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log_path = dir.path().join("w.log");
    let (ux, _) = capture_ux();
    assert!(
        update::start_background_check(
            vrcr_watchdog::maintenance::update::CheckPolicy {
                enabled: false,
                include_prereleases: false
            },
            "1.0",
            ux,
            &Log::to_file(&log_path)
        )
        .is_none()
    );
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
    let slot = std::sync::Arc::clone(&spawned);
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
    assert!(out.logs().contains("updater refreshed for future updates."));
}

#[test]
fn staged_updater_installs_when_no_updater_exists_yet() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("vrcresolver.Updater.next.exe"), "NEW")
        .unwrap_or_else(|e| panic!("{e}"));
    let (ux, _) = capture_ux();
    assert!(update::apply_staged_updater(
        dir.path(),
        &ux,
        &Log::disabled()
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join("vrcresolver.Updater.exe")).unwrap_or_default(),
        "NEW"
    );
}

#[test]
fn staged_updater_ignores_pre_rename_names_and_noops_without_a_stage() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    fs::write(root.join("WKVRCProxy.Updater.exe"), "OLD-REAL-UPDATER")
        .unwrap_or_else(|e| panic!("{e}"));
    fs::write(root.join("WKVRCProxy.Updater.next.exe"), "LAUNCHER")
        .unwrap_or_else(|e| panic!("{e}"));
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
        out.logs()
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
