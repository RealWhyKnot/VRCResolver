use url::Url;
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::trust_gateway;

pub fn is_allowed_host_header(host_header: Option<&str>, port: u16) -> bool {
    let Some(raw) = host_header.filter(|h| !h.trim().is_empty()) else {
        return false;
    };
    let mut host = raw.trim();
    if let Some(colon) = host.rfind(':').filter(|c| *c > 0)
        && host[colon + 1..].parse::<i64>().ok() == Some(i64::from(port))
    {
        host = &host[..colon];
    }
    trust_gateway::is_local_gateway_host(host)
}

pub fn check_target_url(policy: &FirstPartyPolicy, target: &str) -> Result<Url, &'static str> {
    let url = Url::parse(target).map_err(|_| "target_not_absolute")?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("target_scheme_not_http");
    }
    if !policy.is_playback_proxy(&url) {
        return Err("target_not_first_party_playback_proxy");
    }
    Ok(url)
}
