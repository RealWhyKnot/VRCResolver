use super::*;

pub fn thumbprint(der: &[u8]) -> String {
    hex::encode_upper(Sha1::digest(der))
}

pub(super) fn unix(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

pub(super) fn ymd(t: SystemTime) -> Result<(i32, u8, u8), String> {
    let s = UtcStamp::at(t);
    let year = i32::try_from(s.year).map_err(|e| format!("convert certificate year: {e}"))?;
    let month = u8::try_from(s.month).map_err(|e| format!("convert certificate month: {e}"))?;
    let day = u8::try_from(s.day).map_err(|e| format!("convert certificate day: {e}"))?;
    Ok((year, month, day))
}

pub fn generate(now: SystemTime) -> Result<CertMaterial, String> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|e| format!("generate certificate key: {e}"))?;
    let mut params = CertificateParams::new(vec![GATEWAY_HOST.to_owned(), "localhost".to_owned()])
        .map_err(|e| format!("set certificate names: {e}"))?;
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
    let cert = params
        .self_signed(&key)
        .map_err(|e| format!("sign certificate: {e}"))?;
    Ok(CertMaterial {
        cert_der: cert.der().to_vec(),
        key_der: key.serialize_der(),
    })
}

pub fn inspect(der: &[u8]) -> Result<CertFacts, String> {
    let (rest, cert) =
        x509_parser::parse_x509_certificate(der).map_err(|e| format!("decode certificate: {e}"))?;
    if !rest.is_empty() {
        return Err("trailing bytes after certificate".to_owned());
    }
    let cn_matches = cert.subject().iter_common_name().any(|cn| {
        cn.as_str()
            .is_ok_and(|v| v.eq_ignore_ascii_case(GATEWAY_HOST))
    });
    let ours = cn_matches && cert.subject().as_raw() == cert.issuer().as_raw();
    let ca = cert
        .basic_constraints()
        .map_err(|e| format!("read certificate basic constraints: {e}"))?
        .is_some_and(|b| b.value.ca);
    let server_auth_only = cert
        .extended_key_usage()
        .map_err(|e| format!("read certificate key usage: {e}"))?
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
    if let Some(san) = cert
        .subject_alternative_name()
        .map_err(|e| format!("read certificate alternative names: {e}"))?
    {
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
    if !facts
        .ips
        .iter()
        .all(|ip| *ip == IpAddr::V4(Ipv4Addr::LOCALHOST))
    {
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
        .map_err(|e| format!("select TLS protocol versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(material.cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(material.key_der.clone())),
        )
        .map_err(|e| format!("configure TLS certificate: {e}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}
