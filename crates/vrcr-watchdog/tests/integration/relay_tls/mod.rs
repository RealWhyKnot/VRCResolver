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
use vrcr_shared::console::Ux;
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::win::{Elevated, MachineStore};
use vrcr_watchdog::clock;
use vrcr_watchdog::relay::server::RelayDeps;
use vrcr_watchdog::relay::tls::{
    self, BOOTSTRAP_ARG, CERT_FILE, ChildPaths, KEY_FILE, LEGACY_PFX_FILE, LEGACY_PORTS_FILE,
    RelayTls, TlsHost,
};
use vrcr_watchdog::relay::upstream::{ReqwestUpstream, Upstream};
use vrcr_watchdog::relay::{HitchDetector, RelayActivity, RelayConfig, RelayServer};
use vrcr_watchdog::stats::Stats;

use crate::common::ManualClock;
use crate::common::mesh::Capture;

const NOW: u64 = 1_790_769_600;
const YEAR: Duration = Duration::from_secs(365 * 86_400);
const DAY: Duration = Duration::from_secs(86_400);

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(NOW)
}

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (out.ux(), out)
}

fn custom_cert(cn: &str, sans: &[&str], ca: bool, eku: ExtendedKeyUsagePurpose) -> Vec<u8> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap_or_else(|e| panic!("{e}"));
    let mut params =
        CertificateParams::new(sans.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
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
        self.elevations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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

    fn store_remove(
        &self,
        store: MachineStore,
        remove: &dyn Fn(&[u8]) -> bool,
    ) -> io::Result<usize> {
        let mut stores = self.stores.lock().unwrap_or_else(|e| e.into_inner());
        let list = stores.entry(store.name()).or_default();
        let before = list.len();
        list.retain(|c| !remove(c));
        let removed = before - list.len();
        drop(stores);
        Ok(removed)
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
    (
        RelayTls::new(host, &host.paths.state_dir, ux, clock.now()),
        out,
    )
}

async fn https_get(port: u16, sni: &str, root: &[u8], path: &str) -> String {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(root.to_vec()))
        .unwrap_or_else(|e| panic!("{e}"));
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
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
        format!(
            "GET {path} HTTP/1.1\r\nHost: localhost.youtube.com:{port}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::new();
    let _ = tls.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(windows)]
fn x509_ok(der: &[u8]) -> bool {
    tls::inspect(der).is_ok()
}

mod bootstrap_cases;

mod certificates_cases;

mod contracts_cases;

mod removal_cases;
