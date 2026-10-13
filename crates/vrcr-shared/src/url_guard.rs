use std::net::IpAddr;

use url::{Host, Url};

pub fn is_blocked(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 0
                || a == 10
                || a == 127
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return true;
            }
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked(IpAddr::V4(mapped));
            }
            let [a, b, ..] = v6.octets();
            (a & 0xFE) == 0xFC || (a == 0xFE && (b & 0xC0) == 0x80)
        }
    }
}

pub fn is_safe_to_emit(url: Option<&str>) -> bool {
    let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
        return false;
    };
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    match parsed.host() {
        None => false,
        Some(Host::Ipv4(v4)) => !is_blocked(IpAddr::V4(v4)),
        Some(Host::Ipv6(v6)) => !is_blocked(IpAddr::V6(v6)),
        Some(Host::Domain(d)) => {
            !d.is_empty() && !d.eq_ignore_ascii_case("localhost") && !ends_with_ignore_case(d, ".localhost")
        }
    }
}

pub fn is_avpro_compatible_url(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("rtmp://") || lower.starts_with("rtmps://") {
        return false;
    }
    let path = lower.split('?').next().unwrap_or_default();
    !(path.ends_with(".flv") || path.ends_with(".f4v"))
}

pub(crate) fn ends_with_ignore_case(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.is_char_boundary(s.len() - suffix.len())
        && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}
