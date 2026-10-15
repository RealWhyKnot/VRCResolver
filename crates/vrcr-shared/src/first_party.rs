use std::net::IpAddr;

use url::Url;

use crate::url_guard::ends_with_ignore_case;

pub const MAX_SERVER_PROVIDED_ENTRIES: usize = 8;

pub const ALLOWED_PATH_EXTENSIONS: [&str; 18] = [
    "mp4", "m4s", "m4v", "ts", "m3u8", "mpd", "webm", "mkv", "mov", "mp3", "m4a", "aac", "ogg", "opus",
    "wav", "flac", "vtt", "srt",
];

const BUILT_IN_HOSTS: [&str; 2] = ["vrcresolver.com", "whyknot.dev"];
const BUILT_IN_PROXY_PATHS: [&str; 2] = ["/api/proxy", "/api/stream"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FirstPartyPolicy {
    host_families: Vec<String>,
    proxy_paths: Vec<String>,
}

impl FirstPartyPolicy {
    pub fn with_server_provided<H, P>(host_families: Option<&[H]>, proxy_paths: Option<&[P]>) -> Self
    where
        H: AsRef<str>,
        P: AsRef<str>,
    {
        Self {
            host_families: sanitize(host_families, sanitize_host_family),
            proxy_paths: sanitize(proxy_paths, sanitize_proxy_path),
        }
    }

    pub fn host_families(&self) -> &[String] {
        &self.host_families
    }

    pub fn proxy_paths(&self) -> &[String] {
        &self.proxy_paths
    }

    pub fn is_first_party_host(&self, host: &str) -> bool {
        BUILT_IN_HOSTS
            .iter()
            .copied()
            .chain(self.host_families.iter().map(String::as_str))
            .any(|family| host.eq_ignore_ascii_case(family) || is_subdomain_of(host, family))
    }

    pub fn is_playback_proxy_url(&self, url: &str) -> bool {
        Url::parse(url).is_ok_and(|u| self.is_playback_proxy(&u))
    }

    pub fn is_playback_proxy(&self, url: &Url) -> bool {
        url.host_str().is_some_and(|h| self.is_first_party_host(h)) && self.is_playback_proxy_path(url.path())
    }

    pub fn trust_gateway_extension(&self, url: &str) -> &'static str {
        let ext = allowed_extension(url);
        if !ext.is_empty() {
            return ext;
        }
        let Ok(parsed) = Url::parse(url) else {
            return "";
        };
        if !self.is_playback_proxy(&parsed) {
            return "";
        }
        let path = parsed.path();
        let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
        if path.eq_ignore_ascii_case("/api/proxy") && contains_ignore_case(&query, "q=") {
            return "m3u8";
        }
        if contains_ignore_case(path, "manifest") {
            return "m3u8";
        }
        "bin"
    }

    fn is_playback_proxy_path(&self, path: &str) -> bool {
        BUILT_IN_PROXY_PATHS
            .iter()
            .copied()
            .chain(self.proxy_paths.iter().map(String::as_str))
            .any(|prefix| path_is_or_under(path, prefix))
    }
}

pub fn allowed_extension(url: &str) -> &'static str {
    Url::parse(url).map_or("", |u| allowed_extension_from_path(u.path()))
}

pub fn allowed_extension_from_path(path: &str) -> &'static str {
    let Some(dot) = path
        .rfind(['.', '/', '\\'])
        .filter(|&i| path.as_bytes()[i] == b'.')
    else {
        return "";
    };
    let ext = &path[dot + 1..];
    ALLOWED_PATH_EXTENSIONS
        .iter()
        .copied()
        .find(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or("")
}

fn sanitize<S: AsRef<str>>(entries: Option<&[S]>, accept: fn(&str) -> Option<String>) -> Vec<String> {
    entries
        .unwrap_or_default()
        .iter()
        .filter_map(|raw| accept(raw.as_ref()))
        .take(MAX_SERVER_PROVIDED_ENTRIES)
        .collect()
}

fn sanitize_host_family(raw: &str) -> Option<String> {
    let host = raw.trim().trim_end_matches('.').to_lowercase();
    if !(4..=253).contains(&host.len()) || host.parse::<IpAddr>().is_ok() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let valid_label = |l: &&str| {
        (1..=63).contains(&l.len())
            && l.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    };
    (labels.len() >= 2 && labels.iter().all(valid_label)).then_some(host)
}

fn sanitize_proxy_path(raw: &str) -> Option<String> {
    let path = raw.trim();
    let ok = (6..=64).contains(&path.len())
        && path.starts_with("/api/")
        && !path.ends_with('/')
        && !path.contains("..")
        && !path.contains("//")
        && path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'_'));
    ok.then(|| path.to_owned())
}

fn is_subdomain_of(host: &str, family: &str) -> bool {
    host.len() > family.len()
        && ends_with_ignore_case(host, family)
        && host.as_bytes()[host.len() - family.len() - 1] == b'.'
}

fn path_is_or_under(path: &str, prefix: &str) -> bool {
    path.eq_ignore_ascii_case(prefix)
        || (path.len() > prefix.len()
            && path.is_char_boundary(prefix.len())
            && path[..prefix.len()].eq_ignore_ascii_case(prefix)
            && path.as_bytes()[prefix.len()] == b'/')
}

pub(crate) fn contains_ignore_case(hay: &str, needle: &str) -> bool {
    hay.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}
