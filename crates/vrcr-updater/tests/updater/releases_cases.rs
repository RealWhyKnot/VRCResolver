use super::*;

#[test]
fn tag_versions_follow_the_csharp_parser() {
    assert_eq!(
        parse_tag_version("v2026.10.14.0"),
        Ok(Version::parse("2026.10.14.0").unwrap_or(Version::ZERO))
    );
    assert_eq!(
        parse_tag_version("2026.10.14.3-C0A0"),
        Ok(Version::parse("2026.10.14.3").unwrap_or(Version::ZERO))
    );
    assert!(parse_tag_version("v1.2.3-beta").is_err());
    assert!(parse_tag_version("vv1.2").is_err());
    let err = parse_tag_version("garbage").err().unwrap_or_default();
    assert_eq!(err, "Could not parse latest tag: garbage");
}

#[test]
fn stable_release_takes_the_last_zip_and_the_integrity_asset() {
    let doc = release_doc(
        "v2026.10.1.0",
        &json!([
            zip_asset("first.zip"),
            zip_asset("vrcresolver-2026.10.1.0.integrity.tsv"),
            json!({"name": "second.ZIP", "browser_download_url": "https://dl/second.ZIP", "digest": format!("sha256:{SHA_A}")}),
            zip_asset("notes.txt"),
        ]),
    );
    let mut lines = Vec::new();
    let r = select_release(&doc, false, &mut |l| lines.push(l)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.zip_name, "second.ZIP");
    assert_eq!(r.zip_url, "https://dl/second.ZIP");
    assert_eq!(
        r.zip_digest.as_deref(),
        Some(format!("sha256:{SHA_A}").as_str())
    );
    assert_eq!(
        r.integrity_url.as_deref(),
        Some("https://dl/vrcresolver-2026.10.1.0.integrity.tsv")
    );
    assert_eq!(lines.len(), 3);
}

#[test]
fn release_without_a_zip_or_assets_is_refused() {
    let none = release_doc("v1.0.0.0", &json!([zip_asset("a.txt")]));
    assert_eq!(
        select_release(&none, false, &mut no_log()).err().as_deref(),
        Some("No .zip asset on latest release.")
    );
    let missing = json!({"tag_name": "v1.0.0.0"});
    assert!(select_release(&missing, false, &mut no_log()).is_err());
    let bad_tag = release_doc("nightly", &json!([zip_asset("a.zip")]));
    assert!(select_release(&bad_tag, false, &mut no_log()).is_err());
}

#[test]
fn prerelease_mode_picks_the_highest_version() {
    let list = json!([
        {"tag_name": "v2026.9.1.0", "assets": [zip_asset("old.zip")]},
        {"tag_name": "v2026.10.2.0-ab12", "prerelease": true, "assets": [zip_asset("beta.zip")]},
        {"tag_name": "v2026.10.1.0", "assets": [zip_asset("mid.zip")]},
    ]);
    let r = select_release(&list, true, &mut no_log()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.zip_name, "beta.zip");
    assert_eq!(
        select_release(&json!([]), true, &mut no_log())
            .err()
            .as_deref(),
        Some("No releases returned by GitHub.")
    );
}

#[test]
fn integrity_lines_are_matched_by_zip_name() {
    let text =
        format!("# sha\tsize\tname\r\n{SHA_B}\t12\tother.zip\r\n{SHA_A}\t34\tVRCResolver.zip\r\n");
    assert_eq!(
        integrity_sha(&text, "vrcresolver.zip"),
        Some(SHA_A.to_ascii_uppercase())
    );
    assert_eq!(integrity_sha(&text, "missing.zip"), None);
    assert_eq!(integrity_sha("nothex\t1\ta.zip", "a.zip"), None);
    assert_eq!(integrity_sha("", "a.zip"), None);
    assert_eq!(integrity_sha(&text, " "), None);
}

#[test]
fn asset_digest_and_body_fallbacks_parse_only_sha256() {
    assert_eq!(
        asset_digest_sha(Some(&format!(" SHA256:{SHA_A} "))),
        Some(SHA_A.to_ascii_uppercase())
    );
    assert_eq!(asset_digest_sha(Some(&format!("sha512:{SHA_A}"))), None);
    assert_eq!(asset_digest_sha(Some("sha256:abc")), None);
    assert_eq!(asset_digest_sha(None), None);
    let body = format!("Release notes\r\nsha256:  {SHA_B}  \r\nmore");
    assert_eq!(legacy_body_sha(&body), Some(SHA_B.to_ascii_uppercase()));
    assert_eq!(legacy_body_sha("SHA256: short"), None);
}

#[test]
fn expected_sha_prefers_integrity_then_digest_then_body() {
    let doc = release_doc(
        "v1.0.0.0",
        &json!([
            json!({"name": "a.zip", "browser_download_url": "u", "digest": format!("sha256:{SHA_B}")}),
            zip_asset("a.integrity.tsv"),
        ]),
    );
    let mut r = select_release(&doc, false, &mut no_log()).unwrap_or_else(|e| panic!("{e}"));
    r.body = format!("SHA256: {SHA_B}");
    let from_integrity = expected_sha(&r, |_| Ok(format!("{SHA_A}\t1\ta.zip")), &mut no_log());
    assert_eq!(from_integrity, Some(SHA_A.to_ascii_uppercase()));

    let mut logged = Vec::new();
    let from_digest = expected_sha(&r, |_| Err("HTTP 404".to_owned()), &mut |l| logged.push(l));
    assert_eq!(from_digest, Some(SHA_B.to_ascii_uppercase()));
    assert_eq!(
        logged,
        vec!["[updater] integrity asset read failed: HTTP 404".to_owned()]
    );

    r.zip_digest = None;
    r.integrity_url = None;
    assert_eq!(
        expected_sha(&r, |_| panic!("no integrity url"), &mut no_log()),
        Some(SHA_B.to_ascii_uppercase())
    );
    r.body = String::new();
    assert_eq!(expected_sha(&r, |_| Ok(String::new()), &mut no_log()), None);
}

#[test]
fn prerelease_setting_is_read_from_settings_json() {
    assert!(include_prereleases(Some(
        r#"{"maintenance":{"include_prereleases":true}}"#
    )));
    assert!(!include_prereleases(Some(
        r#"{"maintenance":{"include_prereleases":"true"}}"#
    )));
    assert!(!include_prereleases(Some(r#"{"maintenance":{}}"#)));
    assert!(!include_prereleases(Some("not json")));
    assert!(!include_prereleases(None));
}

#[test]
fn tags_become_safe_file_names() {
    assert_eq!(safe_file_token("v2026.10.1.0-AB12"), "v2026.10.1.0-AB12");
    assert_eq!(safe_file_token("../x/y:z"), ".._x_y_z");
}

#[test]
fn prompt_policy_matches_the_csharp_order() {
    assert_eq!(prompt_policy(Some("1"), true), PromptPolicy::Requested);
    assert_eq!(prompt_policy(Some("true"), true), PromptPolicy::Redirected);
    assert_eq!(prompt_policy(None, true), PromptPolicy::Redirected);
    assert_eq!(prompt_policy(None, false), PromptPolicy::Ask);
}

#[test]
fn release_wait_returns_once_processes_and_mutexes_are_gone() {
    let start = Instant::now();
    let clock = Cell::new(start);
    let polls = Cell::new(0u32);
    let left = || {
        polls.set(polls.get() + 1);
        polls.get() < 3
    };
    let sleep = |d: Duration| clock.set(clock.get() + d);
    let now = || clock.get();
    assert_eq!(wait_for_release(&left, &|| true, &sleep, &now), Ok(()));
    assert_eq!(clock.get() - start, Duration::from_millis(200));

    let clock = Cell::new(start);
    let sleep = |d: Duration| clock.set(clock.get() + d);
    let now = || clock.get();
    let waited = wait_for_release(&|| false, &|| false, &sleep, &now).err();
    assert_eq!(waited, Some(RELEASE_BUDGET));
}

#[test]
fn unreadable_file_version_counts_as_zero() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.version = Some("2026.10.1.0-ABCD".to_owned());
    f.latest("v0.0.0.1", "");
    f.answer = false;
    assert_eq!(f.run(), EXIT_NO_SHA);
    assert_eq!(f.out.borrow()[0], "Current version: 0.0.0.0");
}

#[test]
fn release_without_a_checksum_is_refused_before_prompting() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.2.0", "no checksum here");
    assert_eq!(f.run(), EXIT_NO_SHA);
    assert_eq!(
        f.errs(),
        "Refusing to update: release did not provide a zip SHA256."
    );
    assert!(!f.prompted.get());
}

#[test]
fn prerelease_setting_switches_to_the_release_list() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.prereleases = true;
    let list = json!([{"tag_name": "v2026.10.1.0", "assets": [zip_asset("a.zip")]}]);
    f.respond(ANY_RELEASES_URL, 200, "application/json", &list.to_string());
    assert_eq!(f.run(), EXIT_OK);
    assert!(
        f.logs
            .borrow()
            .iter()
            .any(|l| l.ends_with("(include-prereleases=on)"))
    );
}
