use super::*;

#[test]
fn up_to_date_install_exits_cleanly_without_prompting() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.1.0", "");
    assert_eq!(f.run(), EXIT_OK);
    assert_eq!(
        *f.out.borrow(),
        vec![
            "Current version: 2026.10.1.0".to_owned(),
            "You're on the latest version.".to_owned()
        ]
    );
    assert!(!f.prompted.get());
}

#[test]
fn missing_watchdog_fails_with_the_install_dir_hint() {
    let w = world();
    fs::remove_file(w.install.join("vrcresolver.exe")).unwrap_or_else(|e| panic!("{e}"));
    let f = Fake::new(&w.install, &w.temp);
    assert_eq!(f.run(), EXIT_FAILED);
    assert!(
        f.errs()
            .starts_with("Updater failed: vrcresolver.exe was not found next to the updater"),
        "{}",
        f.errs()
    );
}

#[test]
fn declined_prompt_skips_without_touching_the_install() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.2.0", &format!("SHA256: {SHA_A}"));
    f.answer = false;
    assert_eq!(f.run(), EXIT_OK);
    assert_eq!(f.out.borrow().last().map(String::as_str), Some("Skipped."));
    assert_eq!(names(&w.temp), <[std::string::String; 0]>::default());
    assert!(!f.stopped.get());
}

#[test]
fn checksum_mismatch_refuses_and_deletes_the_download() {
    let w = world();
    let (bytes, _) = release_zip(&w.temp);
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.2.0", &format!("SHA256: {SHA_A}"));
    f.zip_bytes = bytes;
    assert_eq!(f.run(), EXIT_SHA_MISMATCH);
    assert!(
        f.errs().starts_with(
            "Refusing to install: SHA256 mismatch.\n  url=https://dl/vrcresolver.zip\n  expected="
        ),
        "{}",
        f.errs()
    );
    assert_eq!(names(&w.temp), <[std::string::String; 0]>::default());
    assert!(!f.stopped.get());
    assert_eq!(read(&w.install.join("vrcresolver.exe")), "old watchdog");
}

#[test]
fn full_update_installs_stages_the_updater_and_relaunches() {
    let w = world();
    let (bytes, sha) = release_zip(&w.temp);
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest(
        "v2026.10.2.0",
        &format!("SHA256: {}", sha.to_ascii_lowercase()),
    );
    f.zip_bytes = bytes;
    assert_eq!(f.run(), EXIT_OK, "{}", f.errs());
    assert!(f.stopped.get());
    assert_eq!(
        *f.launched.borrow(),
        Some((w.install.join("vrcresolver.exe"), w.install.clone()))
    );
    assert_eq!(read(&w.install.join("vrcresolver.exe")), "new watchdog");
    assert_eq!(read(&w.install.join("tools/yt-dlp.exe")), "new shim");
    assert_eq!(
        read(&w.install.join("vrcresolver.Updater.exe")),
        "running updater"
    );
    assert_eq!(
        read(&w.install.join("vrcresolver.Updater.next.exe")),
        "new updater"
    );
    assert!(names(&w.temp).is_empty(), "{:?}", names(&w.temp));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("Update installed. Relaunching watchdog...")
    );
    assert!(
        f.logs
            .borrow()
            .iter()
            .any(|l| l == "[updater] expected-sha256 matched (64 chars)")
    );
}

#[test]
fn non_json_and_error_responses_fail_with_a_clear_message() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.respond(
        STABLE_LATEST_URL,
        200,
        "text/html",
        "<html>maintenance</html>",
    );
    assert_eq!(f.run(), EXIT_FAILED);
    assert!(
        f.errs()
            .contains("returned non-JSON content (Content-Type: text/html)"),
        "{}",
        f.errs()
    );

    let mut f = Fake::new(&w.install, &w.temp);
    f.respond(STABLE_LATEST_URL, 503, "application/json", "{}");
    assert_eq!(f.run(), EXIT_FAILED);
    assert_eq!(
        f.errs(),
        "Updater failed: GitHub releases API returned HTTP 503."
    );
}
