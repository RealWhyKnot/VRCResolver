use super::*;

#[test]
fn remove_child_clears_every_trace() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let clock = ManualClock::utc(NOW);
    let (m, _) = manager(&host, &clock);
    assert!(m.ensure_ready(51234).is_some());
    let foreign = custom_cert(
        "Some Vendor Root",
        &["vendor.example"],
        true,
        ExtendedKeyUsagePurpose::ServerAuth,
    );
    host.seed(MachineStore::Root, foreign.clone());
    host.seed(
        MachineStore::My,
        tls::generate(now())
            .unwrap_or_else(|e| panic!("{e}"))
            .cert_der,
    );
    fs::create_dir_all(&host.paths.program_data_root).unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        host.paths.program_data_root.join(LEGACY_PORTS_FILE),
        "50005\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    fs::create_dir_all(host.paths.legacy_program_data_root.join("sub"))
        .unwrap_or_else(|e| panic!("{e}"));

    let (ux, out) = capture_ux();
    assert_eq!(
        tls::run_remove(&host, &host.paths, &ux),
        0,
        "{}",
        out.text()
    );
    assert_eq!(host.certs(MachineStore::Root), vec![foreign]);
    assert_eq!(
        host.certs(MachineStore::My),
        <[std::vec::Vec<u8>; 0]>::default()
    );
    assert!(!host.paths.state_dir.join(CERT_FILE).exists());
    assert!(!host.paths.state_dir.join(KEY_FILE).exists());
    assert!(!host.paths.program_data_root.exists());
    assert!(!host.paths.legacy_program_data_root.exists());
    assert_eq!(
        *host
            .deleted_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        vec![50005]
    );

    let mut failing = FakeHost::new(Behaviour::RunChild, &dir);
    failing.netsh_fails = true;
    fs::create_dir_all(&failing.paths.state_dir).unwrap_or_else(|e| panic!("{e}"));
    fs::write(failing.paths.state_dir.join("relay_port.txt"), "50006")
        .unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    assert_eq!(tls::run_remove(&failing, &failing.paths, &ux), 1);
    assert!(
        out.text()
            .contains("HTTPS cleanup could not remove sslcert for port 50006: netsh failed")
    );
}
