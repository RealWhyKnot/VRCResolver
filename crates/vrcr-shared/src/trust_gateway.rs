use std::fs;
use std::path::Path;

use url::Url;

use crate::b64;
use crate::first_party::FirstPartyPolicy;

pub const GATEWAY_HOST: &str = "localhost.youtube.com";
const SESSION_MAX: usize = 64;

pub fn build(
    policy: &FirstPartyPolicy,
    port: u32,
    target_url: &str,
    session: Option<&str>,
    scheme: &str,
) -> Option<String> {
    if port <= 1024 || port >= 65536 || !is_allowed_scheme(scheme) || target_url.trim().is_empty() {
        return None;
    }
    if is_local_gateway_url(target_url) || !policy.is_playback_proxy_url(target_url) {
        return None;
    }
    let session = match session.filter(|s| !s.trim().is_empty()) {
        Some(s) => sanitize_session(s),
        None => uuid::Uuid::new_v4().simple().to_string()[..12].to_owned(),
    };
    if session.is_empty() {
        return None;
    }
    let ext = policy.trust_gateway_extension(target_url);
    let suffix = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    Some(format!(
        "{}://{GATEWAY_HOST}:{port}/play/{session}/manifest{suffix}?target={}",
        scheme.to_ascii_lowercase(),
        b64::encode(target_url)
    ))
}

pub fn wrap_for_relay(
    policy: &FirstPartyPolicy,
    state_root: &Path,
    url: &str,
    probe: impl FnOnce(u16) -> bool,
) -> String {
    if url.is_empty() {
        return String::new();
    }
    let Some(port) = read_relay_port(state_root) else {
        return url.to_owned();
    };
    if !probe(port) {
        return url.to_owned();
    }
    build(policy, u32::from(port), url, None, &read_relay_scheme(state_root))
        .unwrap_or_else(|| url.to_owned())
}

pub fn read_relay_port(state_root: &Path) -> Option<u16> {
    let text = fs::read_to_string(state_root.join("relay_port.txt")).ok()?;
    text.trim().parse::<u16>().ok().filter(|p| *p > 1024)
}

pub fn read_relay_scheme(state_root: &Path) -> String {
    fs::read_to_string(state_root.join("relay_scheme.txt"))
        .ok()
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| is_allowed_scheme(t))
        .unwrap_or_else(|| "http".to_owned())
}

pub fn extract_target(local_url: &str) -> Option<String> {
    let uri = Url::parse(local_url.trim()).ok()?;
    if !uri.host_str().is_some_and(is_local_gateway_host) {
        return None;
    }
    let path = uri.path();
    if !path.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("/play/")) {
        return None;
    }
    let encoded = find_query_value(uri.query()?, "target")?;
    if encoded.trim().is_empty() {
        return None;
    }
    let decoded = b64::decode(&encoded)?;
    Url::parse(&decoded).ok()?;
    Some(decoded)
}

pub fn is_local_gateway_host(host: &str) -> bool {
    [GATEWAY_HOST, "localhost", "127.0.0.1"]
        .iter()
        .any(|h| host.eq_ignore_ascii_case(h))
}

pub fn is_allowed_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
}

fn is_local_gateway_url(url: &str) -> bool {
    Url::parse(url).is_ok_and(|u| u.host_str().is_some_and(is_local_gateway_host))
}

fn sanitize_session(session: &str) -> String {
    session
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(SESSION_MAX)
        .collect()
}

fn find_query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (key, _) = part.split_once('=')?;
        if key.is_empty() || !key.eq_ignore_ascii_case(name) {
            return None;
        }
        url::form_urlencoded::parse(part.as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
    })
}
