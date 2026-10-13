mod common;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    PKCS_ECDSA_P256_SHA256,
};
use rustls::pki_types::ServerName;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::win::{Elevated, MachineStore};
use vrcr_watchdog::clock;
use vrcr_watchdog::relay::server::RelayDeps;
use vrcr_watchdog::relay::tls::{
    self, BOOTSTRAP_ARG, CERT_FILE, ChildPaths, KEY_FILE, LEGACY_PFX_FILE, LEGACY_PORTS_FILE, RelayTls,
    TlsHost,
};
use vrcr_watchdog::relay::upstream::{ReqwestUpstream, Upstream};
use vrcr_watchdog::relay::{HitchDetector, RelayActivity, RelayConfig, RelayServer};
use vrcr_watchdog::stats::Stats;

use common::ManualClock;
use common::mesh::Capture;

const NOW: u64 = 1_790_769_600;
const YEAR: Duration = Duration::from_secs(365 * 86_400);
const DAY: Duration = Duration::from_secs(86_400);

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(NOW)
}

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (Ux::new(Box::new(out.clone()), Style::plain(), String::new), out)
}

fn custom_cert(cn: &str, sans: &[&str], ca: bool, eku: ExtendedKeyUsagePurpose) -> Vec<u8> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap_or_else(|e| panic!("{e}"));
    let mut params = CertificateParams::new(sans.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn);
    params.distinguished_name = dn;
    params.is_ca = if ca {
        IsCa::Ca(rcgen::BasicConstraints::Unconstrained)
    } else {
        IsCa::ExplicitNoCa
    };
    params.extended_key_usages = vec![eku];
    params.not_before = rcgen::date_time_ymd(2026, 9, 1);
    params.not_after = rcgen::date_time_ymd(2030, 9, 1);
    params
        .self_signed(&key)
        .unwrap_or_else(|e| panic!("{e}"))
        .der()
        .to_vec()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    RunChild,
    ExitOkWithoutTrust,
    Declined,
    TimedOut,
    ExitCode(u32),
}

struct FakeHost {
    stores: Mutex<HashMap<&'static str, Vec<Vec<u8>>>>,
    behaviour: Behaviour,
    elevations: Mutex<Vec<String>>,
    deleted_bindings: Mutex<Vec<u16>>,
    store_read_fails: bool,
    netsh_fails: bool,
    child_now: SystemTime,
    paths: ChildPaths,
}

impl FakeHost {
    fn new(behaviour: Behaviour, dir: &TempDir) -> FakeHost {
        FakeHost {
            stores: Mutex::new(HashMap::new()),
            behaviour,
            elevations: Mutex::new(Vec::new()),
            deleted_bindings: Mutex::new(Vec::new()),
            store_read_fails: false,
            netsh_fails: false,
            child_now: now(),
            paths: ChildPaths {
                state_dir: dir.path().join("state"),
                program_data_root: dir.path().join("pd").join("vrcresolver"),
                legacy_program_data_root: dir.path().join("pd").join("WKVRCProxy"),
            },
        }
    }

    fn certs(&self, store: MachineStore) -> Vec<Vec<u8>> {
        self.stores
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(store.name())
            .cloned()
            .unwrap_or_default()
    }

    fn seed(&self, store: MachineStore, der: Vec<u8>) {
        self.stores
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(store.name())
            .or_default()
            .push(der);
    }

    fn elevations(&self) -> Vec<String> {
        self.elevations.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl TlsHost for FakeHost {
    fn store_certs(&self, store: MachineStore) -> io::Result<Vec<Vec<u8>>> {
        if self.store_read_fails {
            return Err(io::Error::other("store unavailable"));
        }
        Ok(self.certs(store))
    }

    fn store_add(&self, store: MachineStore, der: &[u8]) -> io::Result<()> {
        if !self.certs(store).iter().any(|c| c == der) {
            self.seed(store, der.to_vec());
        }
        Ok(())
    }

    fn store_remove(&self, store: MachineStore, remove: &dyn Fn(&[u8]) -> bool) -> io::Result<usize> {
        let mut stores = self.stores.lock().unwrap_or_else(|e| e.into_inner());
        let list = stores.entry(store.name()).or_default();
        let before = list.len();
        list.retain(|c| !remove(c));
        Ok(before - list.len())
    }

    fn protect(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = b"sealed:".to_vec();
        out.extend(data.iter().map(|b| b ^ 0x5A));
        Ok(out)
    }

    fn unprotect(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        data.strip_prefix(b"sealed:")
            .map(|d| d.iter().map(|b| b ^ 0x5A).collect())
            .ok_or_else(|| io::Error::other("not sealed"))
    }

    fn elevate(&self, args: &str) -> Elevated {
        self.elevations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(args.to_owned());
        match self.behaviour {
            Behaviour::RunChild => {
                let parts: Vec<String> = args.split(' ').skip(1).map(str::to_owned).collect();
                let (ux, _) = capture_ux();
                let code = tls::run_bootstrap(&parts, self, &self.paths, &ux, self.child_now);
                Elevated::Exited(u32::try_from(code).unwrap_or(99))
            }
            Behaviour::ExitOkWithoutTrust => Elevated::Exited(0),
            Behaviour::Declined => Elevated::Declined,
            Behaviour::TimedOut => Elevated::TimedOut,
            Behaviour::ExitCode(c) => Elevated::Exited(c),
        }
    }

    fn delete_legacy_binding(&self, port: u16) -> io::Result<()> {
        if self.netsh_fails {
            return Err(io::Error::other("netsh failed"));
        }
        self.deleted_bindings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(port);
        Ok(())
    }
}

fn manager<'a>(host: &'a FakeHost, clock: &ManualClock) -> (RelayTls<'a>, Capture) {
    let (ux, out) = capture_ux();
    (RelayTls::new(host, &host.paths.state_dir, ux, clock.now()), out)
}

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
    assert_ne!(tls::thumbprint(&m.cert_der), tls::thumbprint(&other.cert_der));
}

#[test]
fn thumbprint_is_uppercase_sha1() {
    assert_eq!(
        tls::thumbprint(b"abc"),
        "A9993E364706816ABA3E25717850C26C9CD0D89D"
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
            custom_cert("example.com", &[host], false, ExtendedKeyUsagePurpose::ServerAuth),
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
fn disable_switch_accepts_one_or_true() {
    for (v, expected) in [
        (Some("1"), true),
        (Some("true"), true),
        (Some("TRUE"), true),
        (Some("yes"), false),
        (Some("0"), false),
        (Some(""), false),
        (None, false),
    ] {
        assert_eq!(tls::disabled_value(v), expected, "{v:?}");
    }
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
    assert!(host.elevations().is_empty());
    assert!(host.certs(MachineStore::Root).is_empty());
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
    assert!(out.text().contains("UAC declined -- HTTPS relay not configured."));
    assert!(!out.text().contains("falling back"));
    let first = fs::read(declined.paths.state_dir.join(CERT_FILE)).unwrap_or_else(|e| panic!("{e}"));

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
fn privileged_ports_never_prompt() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let clock = ManualClock::utc(NOW);
    let (m, _) = manager(&host, &clock);
    assert!(m.ensure_ready(443).is_none());
    assert!(host.elevations().is_empty());
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
    assert_eq!(host.certs(MachineStore::Root), vec![renewed.clone()]);

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
    host.seed(MachineStore::My, old.cert_der.clone());
    host.seed(MachineStore::My, foreign.clone());
    fs::create_dir_all(&host.paths.state_dir).unwrap_or_else(|e| panic!("{e}"));
    fs::create_dir_all(&host.paths.program_data_root).unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.state_dir.join("relay_last_port.txt"), "50001").unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.state_dir.join("relay_port.txt"), "50002\n").unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        host.paths.program_data_root.join(LEGACY_PORTS_FILE),
        "50001\n49000\n80\nnope\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.program_data_root.join(LEGACY_PFX_FILE), b"pfx").unwrap_or_else(|e| panic!("{e}"));

    let new = tls::generate(now()).unwrap_or_else(|e| panic!("{e}"));
    let (ux, _) = capture_ux();
    let args = vec!["51234".to_owned(), hex::encode(&new.cert_der)];
    assert_eq!(tls::run_bootstrap(&args, &host, &host.paths, &ux, now()), 0);
    assert_eq!(
        host.certs(MachineStore::Root),
        vec![foreign.clone(), new.cert_der.clone()]
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

#[test]
fn bootstrap_child_refuses_bad_input_without_touching_the_store() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let host = FakeHost::new(Behaviour::RunChild, &dir);
    let good = hex::encode(tls::generate(now()).unwrap_or_else(|e| panic!("{e}")).cert_der);
    let ca = hex::encode(custom_cert(
        "localhost.youtube.com",
        &["localhost.youtube.com"],
        true,
        ExtendedKeyUsagePurpose::ServerAuth,
    ));
    for (args, needle) in [
        (vec![], "invalid port."),
        (vec!["80".to_owned(), good.clone()], "invalid port."),
        (vec!["x".to_owned(), good.clone()], "invalid port."),
        (vec!["51234".to_owned()], "invalid certificate."),
        (vec!["51234".to_owned(), "zz".to_owned()], "invalid certificate."),
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
    assert!(host.certs(MachineStore::Root).is_empty());
}

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
        tls::generate(now()).unwrap_or_else(|e| panic!("{e}")).cert_der,
    );
    fs::create_dir_all(&host.paths.program_data_root).unwrap_or_else(|e| panic!("{e}"));
    fs::write(host.paths.program_data_root.join(LEGACY_PORTS_FILE), "50005\n")
        .unwrap_or_else(|e| panic!("{e}"));
    fs::create_dir_all(host.paths.legacy_program_data_root.join("sub")).unwrap_or_else(|e| panic!("{e}"));

    let (ux, out) = capture_ux();
    assert_eq!(tls::run_remove(&host, &host.paths, &ux), 0, "{}", out.text());
    assert_eq!(host.certs(MachineStore::Root), vec![foreign]);
    assert!(host.certs(MachineStore::My).is_empty());
    assert!(!host.paths.state_dir.join(CERT_FILE).exists());
    assert!(!host.paths.state_dir.join(KEY_FILE).exists());
    assert!(!host.paths.program_data_root.exists());
    assert!(!host.paths.legacy_program_data_root.exists());
    assert_eq!(
        *host.deleted_bindings.lock().unwrap_or_else(|e| e.into_inner()),
        vec![50005]
    );

    let mut failing = FakeHost::new(Behaviour::RunChild, &dir);
    failing.netsh_fails = true;
    fs::create_dir_all(&failing.paths.state_dir).unwrap_or_else(|e| panic!("{e}"));
    fs::write(failing.paths.state_dir.join("relay_port.txt"), "50006").unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    assert_eq!(tls::run_remove(&failing, &failing.paths, &ux), 1);
    assert!(
        out.text()
            .contains("HTTPS cleanup could not remove sslcert for port 50006: netsh failed")
    );
}

async fn https_get(port: u16, sni: &str, root: &[u8], path: &str) -> String {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(root.to_vec()))
        .unwrap_or_else(|e| panic!("{e}"));
    let config =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap_or_else(|e| panic!("{e}"))
            .with_root_certificates(roots)
            .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let name = ServerName::try_from(sni.to_owned()).unwrap_or_else(|e| panic!("{e}"));
    let mut tls = connector
        .connect(name, tcp)
        .await
        .unwrap_or_else(|e| panic!("{sni}: {e}"));
    tls.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: localhost.youtube.com:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::new();
    let _ = tls.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out).into_owned()
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
        upstream,
        policy: Arc::new(RwLock::new(FirstPartyPolicy::default())),
        activity: Arc::new(RelayActivity::new()),
        hitch: Arc::new(HitchDetector::new()),
        stats: Arc::new(Stats::new(SystemTime::now())),
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
    let config =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
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
    let roots = vrcr_shared::win::machine_store_certs(MachineStore::Root).unwrap_or_else(|e| panic!("{e}"));
    assert!(!roots.is_empty());
    assert!(roots.iter().any(|der| x509_ok(der)));
}

#[cfg(windows)]
fn x509_ok(der: &[u8]) -> bool {
    tls::inspect(der).is_ok()
}

#[cfg(windows)]
#[test]
fn windows_dpapi_round_trips_for_the_current_user() {
    let secret = tls::generate(now()).unwrap_or_else(|e| panic!("{e}")).key_der;
    let sealed = vrcr_shared::win::dpapi_protect(&secret).unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(sealed, secret);
    assert!(!sealed.windows(secret.len()).any(|w| w == secret.as_slice()));
    assert_eq!(
        vrcr_shared::win::dpapi_unprotect(&sealed).unwrap_or_else(|e| panic!("{e}")),
        secret
    );
    assert!(vrcr_shared::win::dpapi_unprotect(b"not sealed").is_err());
}
