use super::*;

#[test]
fn elevation_outcomes_fall_back_to_http_with_the_csharp_messages() {
    for (behaviour, needle) in [
        (
            Behaviour::ExitOkWithoutTrust,
            "HTTPS setup did not complete; falling back to HTTP relay.",
        ),
        (
            Behaviour::TimedOut,
            "HTTPS elevation child still running after 60s -- falling back to HTTP.",
        ),
    ] {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let host = FakeHost::new(behaviour, &dir);
        let clock = ManualClock::utc(NOW);
        let (m, out) = manager(&host, &clock);
        assert!(m.ensure_ready(51234).is_none());
        assert!(out.text().contains(needle), "{}", out.text());
    }
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::ExitCode(1), &dir);
    let clock = ManualClock::utc(NOW);
    let (m, out) = manager(&host, &clock);
    assert!(m.ensure_ready(51234).is_none());
    assert!(!out.text().contains("falling back"), "{}", out.text());
}

#[test]
fn privileged_ports_never_prompt() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let clock = ManualClock::utc(NOW);
    let (m, _) = manager(&host, &clock);
    assert!(m.ensure_ready(443).is_none());
    assert_eq!(host.elevations(), <[std::string::String; 0]>::default());
}

#[test]
fn bootstrap_child_refuses_bad_input_without_touching_the_store() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let good = hex::encode(
        tls::generate(now())
            .unwrap_or_else(|e| panic!("{e}"))
            .cert_der,
    );
    let ca = hex::encode(custom_cert(
        "localhost.youtube.com",
        &["localhost.youtube.com"],
        true,
        ExtendedKeyUsagePurpose::ServerAuth,
    ));
    for (args, needle) in [
        (vec![], "invalid port."),
        (vec!["80".to_owned(), good.clone()], "invalid port."),
        (vec!["x".to_owned(), good], "invalid port."),
        (vec!["51234".to_owned()], "invalid certificate."),
        (
            vec!["51234".to_owned(), "zz".to_owned()],
            "invalid certificate.",
        ),
        (vec!["51234".to_owned(), ca], "certificate is a CA"),
    ] {
        let (ux, out) = capture_ux();
        assert_eq!(
            tls::run_bootstrap(&args, &host, &host.paths, &ux, now()),
            2,
            "{args:?}"
        );
        assert!(out.text().contains(needle), "{args:?}: {}", out.text());
    }
    assert_eq!(
        host.certs(MachineStore::Root),
        <[std::vec::Vec<u8>; 0]>::default()
    );
}

#[test]
fn removal_check_only_asks_for_elevation_when_something_is_left() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mut host = FakeHost::new(Behaviour::RunChild, &dir);
    let foreign = custom_cert(
        "Some Vendor Root",
        &["vendor.example"],
        true,
        ExtendedKeyUsagePurpose::ServerAuth,
    );
    host.seed(MachineStore::Root, foreign);
    fs::create_dir_all(&host.paths.state_dir).unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.state_dir.join("relay_last_port.txt"), "51234")
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!tls::needs_removal(&host, &host.paths));
    assert_eq!(tls::run_check(&host, &host.paths), tls::CHECK_CLEAN);

    host.seed(
        MachineStore::My,
        tls::generate(now())
            .unwrap_or_else(|e| panic!("{e}"))
            .cert_der,
    );
    assert_eq!(tls::run_check(&host, &host.paths), tls::CHECK_NEEDS_REMOVAL);
    host.stores
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    assert!(!tls::needs_removal(&host, &host.paths));

    fs::create_dir_all(&host.paths.program_data_root).unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.program_data_root.join(LEGACY_PFX_FILE), "x")
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(tls::needs_removal(&host, &host.paths));
    fs::remove_file(host.paths.program_data_root.join(LEGACY_PFX_FILE))
        .unwrap_or_else(|e| panic!("{e}"));
    fs::create_dir_all(&host.paths.legacy_program_data_root).unwrap_or_else(|e| panic!("{e}"));
    assert!(tls::needs_removal(&host, &host.paths));
    fs::remove_dir(&host.paths.legacy_program_data_root).unwrap_or_else(|e| panic!("{e}"));
    assert!(!tls::needs_removal(&host, &host.paths));

    host.store_read_fails = true;
    assert!(tls::needs_removal(&host, &host.paths));
    assert_eq!(host.elevations(), <[std::string::String; 0]>::default());
}
