use super::*;

#[test]
fn generated_certificate_is_a_narrow_server_cert() {
    let m = tls::generate(now()).unwrap_or_else(|e| panic!("{e}"));
    let f = tls::inspect(&m.cert_der).unwrap_or_else(|e| panic!("{e}"));
    assert!(f.ours);
    assert!(!f.ca);
    assert!(f.server_auth_only);
    assert_eq!(f.dns_names, vec!["localhost.youtube.com", "localhost"]);
    assert_eq!(f.ips, vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    let now_s = i64::try_from(NOW).unwrap_or_default();
    assert!(f.not_before <= now_s - 86_400 + 86_400);
    assert!(f.not_before > now_s - 3 * 86_400);
    assert!(f.not_after > now_s + 5 * 365 * 86_400 - 86_400);
    assert!(f.not_after <= now_s + (5 * 365 + 2) * 86_400);
    assert!(tls::validate_for_trust(&m.cert_der, now()).is_ok());
    assert!(tls::server_config(&m).is_ok());
    let other = tls::generate(now()).unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(
        tls::thumbprint(&m.cert_der),
        tls::thumbprint(&other.cert_der)
    );
}

#[test]
fn trust_validation_rejects_anything_broader_than_the_relay_cert() {
    let host = "localhost.youtube.com";
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "CA",
            custom_cert(host, &[host], true, ExtendedKeyUsagePurpose::ServerAuth),
        ),
        (
            "not limited to server authentication",
            custom_cert(host, &[host], false, ExtendedKeyUsagePurpose::CodeSigning),
        ),
        (
            "other than the local relay",
            custom_cert(
                host,
                &[host, "accounts.google.com"],
                false,
                ExtendedKeyUsagePurpose::ServerAuth,
            ),
        ),
        (
            "other than the local relay",
            custom_cert(
                host,
                &["*.youtube.com"],
                false,
                ExtendedKeyUsagePurpose::ServerAuth,
            ),
        ),
        (
            "not a self-issued",
            custom_cert(
                "example.com",
                &[host],
                false,
                ExtendedKeyUsagePurpose::ServerAuth,
            ),
        ),
        ("", b"definitely not a certificate".to_vec()),
    ];
    for (needle, der) in cases {
        let err = tls::validate_for_trust(&der, now())
            .err()
            .unwrap_or_else(|| panic!("{needle} accepted"));
        assert!(err.contains(needle), "{needle}: {err}");
    }
    let expiring = tls::generate(now() - 5 * YEAR + 10 * DAY).unwrap_or_else(|e| panic!("{e}"));
    let err = tls::validate_for_trust(&expiring.cert_der, now())
        .err()
        .unwrap_or_default();
    assert!(err.contains("expires within 30 days"), "{err}");
    let future = tls::generate(now() + 2 * YEAR).unwrap_or_else(|e| panic!("{e}"));
    let err = tls::validate_for_trust(&future.cert_der, now())
        .err()
        .unwrap_or_default();
    assert!(err.contains("longer than expected"), "{err}");
}

#[test]
fn first_run_elevates_once_then_reuses_the_trusted_cert() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let clock = ManualClock::utc(NOW);
    let (m, out) = manager(&host, &clock);
    assert!(m.ensure_ready(51234).is_some());
    let elevations = host.elevations();
    assert_eq!(elevations.len(), 1);
    assert!(elevations[0].starts_with(&format!("{BOOTSTRAP_ARG} 51234 ")));
    let text = out.text();
    assert!(
        text.contains("configuring HTTPS for localhost.youtube.com -- UAC prompt incoming."),
        "{text}"
    );
    assert!(
        text.contains("HTTPS ready for localhost.youtube.com:51234"),
        "{text}"
    );

    let cert = fs::read(host.paths.state_dir.join(CERT_FILE)).unwrap_or_else(|e| panic!("{e}"));
    let sealed = fs::read(host.paths.state_dir.join(KEY_FILE)).unwrap_or_else(|e| panic!("{e}"));
    assert!(sealed.starts_with(b"sealed:"));
    assert_eq!(host.certs(MachineStore::Root), vec![cert.clone()]);

    let (m2, out2) = manager(&host, &clock);
    assert!(m2.ensure_ready(51300).is_some());
    assert_eq!(host.elevations().len(), 1);
    assert!(out2.text().is_empty(), "{}", out2.text());
    assert_eq!(
        fs::read(host.paths.state_dir.join(CERT_FILE)).unwrap_or_default(),
        cert
    );
}

#[test]
fn disable_switch_skips_https_without_elevating_or_writing_a_cert() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let clock = ManualClock::utc(NOW);
    let (m, out) = manager(&host, &clock);
    assert!(m.disabled(true).ensure_ready(51234).is_none());
    assert_eq!(host.elevations(), <[std::string::String; 0]>::default());
    assert_eq!(
        host.certs(MachineStore::Root),
        <[std::vec::Vec<u8>; 0]>::default()
    );
    assert!(!host.paths.state_dir.join(CERT_FILE).exists());
    assert!(out.text().is_empty(), "{}", out.text());
}

#[test]
fn declined_uac_keeps_the_cert_for_the_next_attempt() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let clock = ManualClock::utc(NOW);
    let declined = FakeHost::new(Behaviour::Declined, &dir);
    let (m, out) = manager(&declined, &clock);
    assert!(m.ensure_ready(51234).is_none());
    assert!(
        out.text()
            .contains("UAC declined -- HTTPS relay not configured.")
    );
    assert!(!out.text().contains("falling back"));
    let first =
        fs::read(declined.paths.state_dir.join(CERT_FILE)).unwrap_or_else(|e| panic!("{e}"));

    let accepting = FakeHost::new(Behaviour::RunChild, &dir);
    let (m, _) = manager(&accepting, &clock);
    assert!(m.ensure_ready(51234).is_some());
    let hex_arg = accepting.elevations()[0]
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .to_owned();
    assert_eq!(hex_arg, hex::encode(&first));
}

#[test]
fn unreadable_root_store_warns_then_still_tries() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mut host = FakeHost::new(Behaviour::ExitOkWithoutTrust, &dir);
    host.store_read_fails = true;
    let clock = ManualClock::utc(NOW);
    let (m, out) = manager(&host, &clock);
    assert!(m.ensure_ready(51234).is_none());
    let text = out.text();
    assert!(
        text.contains("HTTPS readiness check failed: store unavailable"),
        "{text}"
    );
    assert!(
        text.contains("HTTPS verification failed after elevation: store unavailable"),
        "{text}"
    );
    assert_eq!(host.elevations().len(), 1);
}

#[test]
fn expiring_or_tampered_material_is_regenerated() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mut host = FakeHost::new(Behaviour::RunChild, &dir);
    let early = now() - 5 * YEAR + 20 * DAY;
    host.child_now = early;
    let clock = ManualClock::at(early);
    let (m, _) = manager(&host, &clock);
    assert!(m.ensure_ready(51234).is_some());
    let old = fs::read(host.paths.state_dir.join(CERT_FILE)).unwrap_or_default();
    drop(m);

    host.child_now = now();
    let later = ManualClock::utc(NOW);
    let (m, _) = manager(&host, &later);
    assert!(m.load().is_none());
    assert!(m.ensure_ready(51234).is_some());
    let renewed = fs::read(host.paths.state_dir.join(CERT_FILE)).unwrap_or_default();
    assert_ne!(old, renewed);
    assert_eq!(host.certs(MachineStore::Root), vec![renewed]);

    fs::write(host.paths.state_dir.join(KEY_FILE), b"garbage").unwrap_or_else(|e| panic!("{e}"));
    let (m, _) = manager(&host, &later);
    assert!(m.load().is_none());
    let other = tls::generate(now()).unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        host.paths.state_dir.join(KEY_FILE),
        host.protect(&other.key_der).unwrap_or_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        m.load().is_none(),
        "a key that does not match the cert must not load"
    );
}

#[test]
fn bootstrap_child_trusts_only_the_new_cert_and_sweeps_legacy_state() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let old = tls::generate(now() - YEAR).unwrap_or_else(|e| panic!("{e}"));
    let foreign = custom_cert(
        "Some Vendor Root",
        &["vendor.example"],
        true,
        ExtendedKeyUsagePurpose::ServerAuth,
    );
    host.seed(MachineStore::Root, old.cert_der.clone());
    host.seed(MachineStore::Root, foreign.clone());
    host.seed(MachineStore::My, old.cert_der);
    host.seed(MachineStore::My, foreign.clone());
    fs::create_dir_all(&host.paths.state_dir).unwrap_or_else(|e| panic!("{e}"));
    fs::create_dir_all(&host.paths.program_data_root).unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.state_dir.join("relay_last_port.txt"), "50001")
        .unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.state_dir.join("relay_port.txt"), "50002\n")
        .unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        host.paths.program_data_root.join(LEGACY_PORTS_FILE),
        "50001\n49000\n80\nnope\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.program_data_root.join(LEGACY_PFX_FILE), b"pfx")
        .unwrap_or_else(|e| panic!("{e}"));

    let new = tls::generate(now()).unwrap_or_else(|e| panic!("{e}"));
    let (ux, _) = capture_ux();
    let args = vec!["51234".to_owned(), hex::encode(&new.cert_der)];
    assert_eq!(tls::run_bootstrap(&args, &host, &host.paths, &ux, now()), 0);
    assert_eq!(
        host.certs(MachineStore::Root),
        vec![foreign.clone(), new.cert_der]
    );
    assert_eq!(host.certs(MachineStore::My), vec![foreign]);
    let mut deleted = host
        .deleted_bindings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    deleted.sort_unstable();
    assert_eq!(deleted, vec![49000, 50001, 50002]);
    assert!(!host.paths.program_data_root.join(LEGACY_PFX_FILE).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_serves_https_with_the_generated_cert() {
    let material = tls::generate(SystemTime::now()).unwrap_or_else(|e| panic!("{e}"));
    let config = tls::server_config(&material).unwrap_or_else(|e| panic!("{e}"));
    let tls_client =
        (*vrcr_watchdog::mesh::connect::platform_tls().unwrap_or_else(|e| panic!("{e}"))).clone();
    let upstream: Arc<dyn Upstream> =
        Arc::new(ReqwestUpstream::new(tls_client).unwrap_or_else(|e| panic!("{e}")));
    let out = Capture::default();
    let deps = RelayDeps {
        recovery: Default::default(),
        upstream,
        policy: Arc::new(RwLock::new(FirstPartyPolicy::default())),
        activity: Arc::new(RelayActivity::new()),
        hitch: Arc::new(HitchDetector::new()),
        stats: Arc::new(Stats::new(SystemTime::now())),
        manifests: Arc::default(),
        now: clock::system(),
        ux: Ux::new(Box::new(out.clone()), Style::plain(), String::new),
        log: Log::disabled(),
    };
    let mut cfg = RelayConfig::http(0);
    cfg.scheme = "https".to_owned();
    cfg.tls = Some(config);
    let server = RelayServer::start(cfg, deps)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(server.scheme(), "https");
    for sni in ["localhost.youtube.com", "localhost"] {
        let reply = https_get(server.port(), sni, &material.cert_der, "/not-play/x").await;
        assert!(reply.starts_with("HTTP/1.1 404"), "{sni}: {reply}");
    }
    let reply = https_get(
        server.port(),
        "localhost.youtube.com",
        &material.cert_der,
        "/play/x/seg.ts",
    )
    .await;
    assert!(reply.starts_with("HTTP/1.1 400"), "{reply}");

    let stranger = tls::generate(SystemTime::now()).unwrap_or_else(|e| panic!("{e}"));
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(stranger.cert_der))
        .unwrap_or_else(|e| panic!("{e}"));
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap_or_else(|e| panic!("{e}"))
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let name = ServerName::try_from("localhost.youtube.com").unwrap_or_else(|e| panic!("{e}"));
    assert!(
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .is_err()
    );
    let mut plain = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    plain
        .write_all(b"GET /not-play/x HTTP/1.1\r\nHost: localhost.youtube.com\r\n\r\n")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), plain.read_to_end(&mut buf)).await;
    assert!(read.is_ok());
    assert!(!String::from_utf8_lossy(&buf).contains("404"));
    server.stop().await;
}

#[cfg(windows)]
#[test]
fn windows_root_store_is_readable_without_elevation() {
    let roots =
        vrcr_shared::win::machine_store_certs(MachineStore::Root).unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(roots, <[std::vec::Vec<u8>; 0]>::default());
    assert!(roots.iter().any(|der| x509_ok(der)));
}

#[cfg(windows)]
#[test]
fn windows_dpapi_round_trips_for_the_current_user() {
    let secret = tls::generate(now())
        .unwrap_or_else(|e| panic!("{e}"))
        .key_der;
    let sealed = vrcr_shared::win::dpapi_protect(&secret).unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(sealed, secret);
    assert!(!sealed.windows(secret.len()).any(|w| w == secret.as_slice()));
    assert_eq!(
        vrcr_shared::win::dpapi_unprotect(&sealed).unwrap_or_else(|e| panic!("{e}")),
        secret
    );
    assert!(vrcr_shared::win::dpapi_unprotect(b"not sealed").is_err());
}
