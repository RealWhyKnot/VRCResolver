use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256, SanType, date_time_ymd,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha1::{Digest, Sha1};
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::legacy;
use vrcr_shared::logutil::UtcStamp;
use vrcr_shared::trust_gateway::GATEWAY_HOST;
use vrcr_shared::win::{Elevated, MachineStore};
use x509_parser::extensions::GeneralName;

use crate::clock::Now;

pub use vrcr_shared::relay_tls::{BOOTSTRAP_ARG, CHECK_ARG, CHECK_CLEAN, CHECK_NEEDS_REMOVAL, REMOVE_ARG};

pub const CERT_FILE: &str = "relay_tls_cert.der";
pub const KEY_FILE: &str = "relay_tls_key.bin";
pub const LEGACY_PORTS_FILE: &str = "localhost-youtube-relay-ports.txt";
pub const LEGACY_PFX_FILE: &str = "localhost-youtube-relay.pfx";
pub const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(60);

const DAY: Duration = Duration::from_secs(86_400);
const MIN_REMAINING: Duration = Duration::from_secs(30 * 86_400);
const VALIDITY_DAYS: u64 = 5 * 365 + 1;

pub trait TlsHost: Send + Sync {
    fn store_certs(&self, store: MachineStore) -> io::Result<Vec<Vec<u8>>>;
    fn store_add(&self, store: MachineStore, der: &[u8]) -> io::Result<()>;
    fn store_remove(&self, store: MachineStore, remove: &dyn Fn(&[u8]) -> bool) -> io::Result<usize>;
    fn protect(&self, data: &[u8]) -> io::Result<Vec<u8>>;
    fn unprotect(&self, data: &[u8]) -> io::Result<Vec<u8>>;
    fn elevate(&self, args: &str) -> Elevated;
    fn delete_legacy_binding(&self, port: u16) -> io::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertMaterial {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertFacts {
    pub ours: bool,
    pub ca: bool,
    pub server_auth_only: bool,
    pub not_before: i64,
    pub not_after: i64,
    pub dns_names: Vec<String>,
    pub ips: Vec<IpAddr>,
}

pub fn valid_port(port: u16) -> bool {
    port > 1024
}

pub fn disabled_by_env() -> bool {
    disabled_value(legacy::env_with_fallback("DISABLE_LOCAL_RELAY_HTTPS").as_deref())
}

pub fn disabled_value(value: Option<&str>) -> bool {
    matches!(value, Some(v) if v == "1" || v.eq_ignore_ascii_case("true"))
}

pub fn thumbprint(der: &[u8]) -> String {
    hex::encode_upper(Sha1::digest(der))
}

fn unix(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

fn ymd(t: SystemTime) -> Result<(i32, u8, u8), String> {
    let s = UtcStamp::at(t);
    let year = i32::try_from(s.year).map_err(|e| e.to_string())?;
    let month = u8::try_from(s.month).map_err(|e| e.to_string())?;
    let day = u8::try_from(s.day).map_err(|e| e.to_string())?;
    Ok((year, month, day))
}

pub fn generate(now: SystemTime) -> Result<CertMaterial, String> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|e| e.to_string())?;
    let mut params = CertificateParams::new(vec![GATEWAY_HOST.to_owned(), "localhost".to_owned()])
        .map_err(|e| e.to_string())?;
    params
        .subject_alt_names
        .push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, GATEWAY_HOST);
    params.distinguished_name = dn;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let (y, m, d) = ymd(now - DAY)?;
    params.not_before = date_time_ymd(y, m, d);
    let (y, m, d) = ymd(now + DAY * u32::try_from(VALIDITY_DAYS).unwrap_or(u32::MAX))?;
    params.not_after = date_time_ymd(y, m, d);
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok(CertMaterial {
        cert_der: cert.der().to_vec(),
        key_der: key.serialize_der(),
    })
}

pub fn inspect(der: &[u8]) -> Result<CertFacts, String> {
    let (rest, cert) = x509_parser::parse_x509_certificate(der).map_err(|e| e.to_string())?;
    if !rest.is_empty() {
        return Err("trailing bytes after certificate".to_owned());
    }
    let cn_matches = cert
        .subject()
        .iter_common_name()
        .any(|cn| cn.as_str().is_ok_and(|v| v.eq_ignore_ascii_case(GATEWAY_HOST)));
    let ours = cn_matches && cert.subject().as_raw() == cert.issuer().as_raw();
    let ca = cert
        .basic_constraints()
        .map_err(|e| e.to_string())?
        .is_some_and(|b| b.value.ca);
    let server_auth_only = cert
        .extended_key_usage()
        .map_err(|e| e.to_string())?
        .is_some_and(|e| {
            let u = e.value;
            u.server_auth
                && !u.any
                && !u.client_auth
                && !u.code_signing
                && !u.email_protection
                && !u.time_stamping
                && !u.ocsp_signing
                && u.other.is_empty()
        });
    let mut dns_names = Vec::new();
    let mut ips = Vec::new();
    if let Some(san) = cert.subject_alternative_name().map_err(|e| e.to_string())? {
        for name in &san.value.general_names {
            match name {
                GeneralName::DNSName(d) => dns_names.push((*d).to_owned()),
                GeneralName::IPAddress(raw) => match raw.len() {
                    4 => ips.push(IpAddr::from(<[u8; 4]>::try_from(*raw).unwrap_or_default())),
                    16 => ips.push(IpAddr::from(<[u8; 16]>::try_from(*raw).unwrap_or_default())),
                    _ => return Err("malformed IP address in subjectAltName".to_owned()),
                },
                _ => return Err("unexpected subjectAltName entry".to_owned()),
            }
        }
    }
    Ok(CertFacts {
        ours,
        ca,
        server_auth_only,
        not_before: cert.validity().not_before.timestamp(),
        not_after: cert.validity().not_after.timestamp(),
        dns_names,
        ips,
    })
}

pub fn is_ours(der: &[u8]) -> bool {
    inspect(der).is_ok_and(|f| f.ours)
}

pub fn usable(facts: &CertFacts, now: SystemTime) -> bool {
    facts.ours && facts.not_after > unix(now + MIN_REMAINING)
}

pub fn validate_for_trust(der: &[u8], now: SystemTime) -> Result<CertFacts, String> {
    let facts = inspect(der)?;
    if !facts.ours {
        return Err(format!(
            "certificate is not a self-issued {GATEWAY_HOST} certificate"
        ));
    }
    if facts.ca {
        return Err("certificate is a CA".to_owned());
    }
    if !facts.server_auth_only {
        return Err("certificate is not limited to server authentication".to_owned());
    }
    let allowed_dns =
        |d: &String| d.eq_ignore_ascii_case(GATEWAY_HOST) || d.eq_ignore_ascii_case("localhost");
    if facts.dns_names.is_empty() || !facts.dns_names.iter().all(allowed_dns) {
        return Err("certificate names hosts other than the local relay".to_owned());
    }
    if !facts.ips.iter().all(|ip| *ip == IpAddr::V4(Ipv4Addr::LOCALHOST)) {
        return Err("certificate names addresses other than 127.0.0.1".to_owned());
    }
    if !usable(&facts, now) {
        return Err("certificate expires within 30 days".to_owned());
    }
    let max_days = i64::try_from(VALIDITY_DAYS + 2).unwrap_or(i64::MAX);
    if facts.not_after > unix(now) + max_days * 86_400 {
        return Err("certificate validity is longer than expected".to_owned());
    }
    Ok(facts)
}

pub fn server_config(material: &CertMaterial) -> Result<Arc<rustls::ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(material.cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(material.key_der.clone())),
        )
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub struct RelayTls<'a> {
    host: &'a dyn TlsHost,
    state_dir: PathBuf,
    ux: Ux,
    now: Now,
    disabled: bool,
}

impl<'a> RelayTls<'a> {
    pub fn new(host: &'a dyn TlsHost, state_dir: &Path, ux: Ux, now: Now) -> RelayTls<'a> {
        RelayTls {
            host,
            state_dir: state_dir.to_path_buf(),
            ux,
            now,
            disabled: disabled_by_env(),
        }
    }

    pub fn disabled(mut self, disabled: bool) -> RelayTls<'a> {
        self.disabled = disabled;
        self
    }

    pub fn ensure_ready(&self, port: u16) -> Option<Arc<rustls::ServerConfig>> {
        if self.disabled || !valid_port(port) {
            return None;
        }
        let material = self.load_or_create()?;
        let thumb = thumbprint(&material.cert_der);
        match self.trusted(&thumb) {
            Ok(true) => return self.config(&material),
            Ok(false) => {}
            Err(e) => self
                .ux
                .warn(Component::Relay, &format!("HTTPS readiness check failed: {e}")),
        }
        self.ux.write(
            Component::Relay,
            "configuring HTTPS for localhost.youtube.com -- UAC prompt incoming.",
        );
        let args = format!("{BOOTSTRAP_ARG} {port} {}", hex::encode(&material.cert_der));
        match self.host.elevate(&args) {
            Elevated::Exited(0) => {}
            Elevated::Exited(_) => return None,
            Elevated::Declined => {
                self.ux
                    .write(Component::Relay, "UAC declined -- HTTPS relay not configured.");
                return None;
            }
            Elevated::TimedOut => {
                self.ux.warn(
                    Component::Relay,
                    "HTTPS elevation child still running after 60s -- falling back to HTTP.",
                );
                return None;
            }
            Elevated::Failed(e) => {
                self.ux
                    .warn(Component::Relay, &format!("HTTPS elevation error: {e}"));
                return None;
            }
        }
        match self.trusted(&thumb) {
            Ok(true) => {
                self.ux.write(
                    Component::Relay,
                    &format!("HTTPS ready for localhost.youtube.com:{port}"),
                );
                return self.config(&material);
            }
            Ok(false) => {}
            Err(e) => self.ux.warn(
                Component::Relay,
                &format!("HTTPS verification failed after elevation: {e}"),
            ),
        }
        self.ux.warn(
            Component::Relay,
            "HTTPS setup did not complete; falling back to HTTP relay.",
        );
        None
    }

    fn config(&self, material: &CertMaterial) -> Option<Arc<rustls::ServerConfig>> {
        match server_config(material) {
            Ok(c) => Some(c),
            Err(e) => {
                self.ux.warn(
                    Component::Relay,
                    &format!("HTTPS certificate could not be loaded: {e}"),
                );
                None
            }
        }
    }

    fn trusted(&self, thumb: &str) -> io::Result<bool> {
        Ok(self
            .host
            .store_certs(MachineStore::Root)?
            .iter()
            .any(|der| thumbprint(der) == thumb))
    }

    pub fn load(&self) -> Option<CertMaterial> {
        let cert_der = fs::read(self.state_dir.join(CERT_FILE)).ok()?;
        let sealed = fs::read(self.state_dir.join(KEY_FILE)).ok()?;
        let key_der = self.host.unprotect(&sealed).ok()?;
        let facts = inspect(&cert_der).ok()?;
        if !usable(&facts, (self.now)()) {
            return None;
        }
        let material = CertMaterial { cert_der, key_der };
        server_config(&material).ok()?;
        Some(material)
    }

    fn load_or_create(&self) -> Option<CertMaterial> {
        if let Some(m) = self.load() {
            return Some(m);
        }
        let material = match generate((self.now)()) {
            Ok(m) => m,
            Err(e) => {
                self.ux.warn(
                    Component::Relay,
                    &format!("HTTPS certificate could not be created: {e}"),
                );
                return None;
            }
        };
        if let Err(e) = self.save(&material) {
            self.ux.warn(
                Component::Relay,
                &format!("HTTPS certificate could not be saved: {e}"),
            );
        }
        Some(material)
    }

    fn save(&self, material: &CertMaterial) -> io::Result<()> {
        fs::create_dir_all(&self.state_dir)?;
        let sealed = self.host.protect(&material.key_der)?;
        write_atomic(&self.state_dir.join(KEY_FILE), &sealed)?;
        write_atomic(&self.state_dir.join(CERT_FILE), &material.cert_der)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}

pub struct ChildPaths {
    pub state_dir: PathBuf,
    pub program_data_root: PathBuf,
    pub legacy_program_data_root: PathBuf,
}

pub fn run_bootstrap(
    args: &[String],
    host: &dyn TlsHost,
    paths: &ChildPaths,
    ux: &Ux,
    now: SystemTime,
) -> i32 {
    let port_ok = args
        .first()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .is_some_and(valid_port);
    if !port_ok {
        ux.error(Component::Relay, "HTTPS setup failed: invalid port.");
        return 2;
    }
    let der = match args.get(1).map(|h| hex::decode(h.trim())) {
        Some(Ok(der)) => der,
        _ => {
            ux.error(Component::Relay, "HTTPS setup failed: invalid certificate.");
            return 2;
        }
    };
    if let Err(e) = validate_for_trust(&der, now) {
        ux.error(Component::Relay, &format!("HTTPS setup failed: {e}"));
        return 2;
    }
    let keep = thumbprint(&der);
    let trusted = host
        .store_add(MachineStore::Root, &der)
        .and_then(|()| host.store_remove(MachineStore::Root, &|c| is_ours(c) && thumbprint(c) != keep));
    if let Err(e) = trusted {
        ux.error(Component::Relay, &format!("HTTPS setup failed: {e}"));
        return 1;
    }
    if let Err(e) = host.store_remove(MachineStore::My, &is_ours) {
        ux.warn(
            Component::Relay,
            &format!("HTTPS setup could not remove the old personal certificate: {e}"),
        );
    }
    for p in legacy_ports(paths) {
        let _ = host.delete_legacy_binding(p);
    }
    let _ = remove_file_if_present(&paths.program_data_root.join(LEGACY_PFX_FILE));
    0
}

pub fn needs_removal(host: &dyn TlsHost, paths: &ChildPaths) -> bool {
    let legacy_files = [
        paths.program_data_root.join(LEGACY_PFX_FILE),
        paths.program_data_root.join(LEGACY_PORTS_FILE),
    ];
    if legacy_files.iter().any(|p| p.exists()) || paths.legacy_program_data_root.is_dir() {
        return true;
    }
    [MachineStore::My, MachineStore::Root].into_iter().any(|store| {
        host.store_certs(store)
            .map_or(true, |certs| certs.iter().any(|c| is_ours(c)))
    })
}

pub fn run_check(host: &dyn TlsHost, paths: &ChildPaths) -> i32 {
    if needs_removal(host, paths) {
        CHECK_NEEDS_REMOVAL
    } else {
        CHECK_CLEAN
    }
}

pub fn run_remove(host: &dyn TlsHost, paths: &ChildPaths, ux: &Ux) -> i32 {
    let mut errors = 0;
    for port in legacy_ports(paths) {
        if let Err(e) = host.delete_legacy_binding(port) {
            errors += 1;
            ux.warn(
                Component::Relay,
                &format!("HTTPS cleanup could not remove sslcert for port {port}: {e}"),
            );
        }
    }
    let mut step = |result: io::Result<()>, what: &str| {
        if let Err(e) = result {
            errors += 1;
            ux.warn(Component::Relay, &format!("HTTPS cleanup could not {what}: {e}"));
        }
    };
    step(
        host.store_remove(MachineStore::My, &is_ours).map(drop),
        "remove personal certificate",
    );
    step(
        host.store_remove(MachineStore::Root, &is_ours).map(drop),
        "remove trusted root certificate",
    );
    step(
        remove_file_if_present(&paths.program_data_root.join(LEGACY_PFX_FILE)),
        "delete legacy certificate file",
    );
    step(
        remove_file_if_present(&paths.program_data_root.join(LEGACY_PORTS_FILE)),
        "delete ports file",
    );
    step(
        remove_file_if_present(&paths.state_dir.join(KEY_FILE)),
        "delete certificate key",
    );
    step(
        remove_file_if_present(&paths.state_dir.join(CERT_FILE)),
        "delete certificate file",
    );
    if fs::read_dir(&paths.program_data_root).is_ok_and(|mut d| d.next().is_none()) {
        let _ = fs::remove_dir(&paths.program_data_root);
    }
    if paths.legacy_program_data_root.is_dir() {
        let _ = fs::remove_dir_all(&paths.legacy_program_data_root);
    }
    i32::from(errors != 0)
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub fn legacy_ports(paths: &ChildPaths) -> Vec<u16> {
    let parse = |s: &str| s.trim().parse::<u16>().ok().filter(|p| valid_port(*p));
    let mut ports = Vec::new();
    for name in ["relay_last_port.txt", "relay_port.txt"] {
        if let Some(p) = fs::read_to_string(paths.state_dir.join(name))
            .ok()
            .as_deref()
            .and_then(parse)
        {
            ports.push(p);
        }
    }
    if let Ok(text) = fs::read_to_string(paths.program_data_root.join(LEGACY_PORTS_FILE)) {
        ports.extend(text.lines().filter_map(parse));
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

#[cfg(windows)]
pub struct WindowsTlsHost {
    exe: PathBuf,
}

#[cfg(windows)]
impl WindowsTlsHost {
    pub fn current() -> io::Result<WindowsTlsHost> {
        Ok(WindowsTlsHost {
            exe: std::env::current_exe()?,
        })
    }
}

#[cfg(windows)]
impl TlsHost for WindowsTlsHost {
    fn store_certs(&self, store: MachineStore) -> io::Result<Vec<Vec<u8>>> {
        vrcr_shared::win::machine_store_certs(store)
    }

    fn store_add(&self, store: MachineStore, der: &[u8]) -> io::Result<()> {
        vrcr_shared::win::machine_store_add(store, der)
    }

    fn store_remove(&self, store: MachineStore, remove: &dyn Fn(&[u8]) -> bool) -> io::Result<usize> {
        vrcr_shared::win::machine_store_remove(store, remove)
    }

    fn protect(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        vrcr_shared::win::dpapi_protect(data)
    }

    fn unprotect(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        vrcr_shared::win::dpapi_unprotect(data)
    }

    fn elevate(&self, args: &str) -> Elevated {
        vrcr_shared::win::run_elevated(&self.exe, args, BOOTSTRAP_TIMEOUT)
    }

    fn delete_legacy_binding(&self, port: u16) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("netsh")
            .args(["http", "delete", "sslcert", &format!("ipport=127.0.0.1:{port}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                return Err(io::Error::new(io::ErrorKind::TimedOut, "netsh timed out"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
