use std::sync::LazyLock;

use regex_lite::Regex;
use serde_json::Value;
use vrcr_shared::release::{Version, pick_highest_entry};

static SHA256_LINE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?mi)^SHA256:[ \t]*([0-9A-Fa-f]{64})[ \t]*\r?$").ok());
static DEV_SUFFIX: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"-[A-Fa-f0-9]{4}$").ok());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestRelease {
    pub version: Version,
    pub tag: String,
    pub zip_url: String,
    pub zip_name: String,
    pub zip_digest: Option<String>,
    pub integrity_url: Option<String>,
    pub body: String,
}

pub fn parse_tag_version(tag: &str) -> Result<Version, String> {
    let numeric = tag.strip_prefix('v').unwrap_or(tag);
    let numeric = match DEV_SUFFIX.as_ref() {
        Some(re) => re.replace(numeric, "").into_owned(),
        None => numeric.to_owned(),
    };
    Version::parse(&numeric).ok_or_else(|| format!("Could not parse latest tag: {tag}"))
}

fn text(entry: &Value, key: &str) -> String {
    entry
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn ends_with_ignore_case(name: &str, suffix: &str) -> bool {
    name.len() >= suffix.len()
        && name.is_char_boundary(name.len() - suffix.len())
        && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

pub fn select_release(
    doc: &Value,
    include_prereleases: bool,
    log: &mut dyn FnMut(String),
) -> Result<LatestRelease, String> {
    let entry = if include_prereleases {
        pick_highest_entry(doc).ok_or_else(|| "No releases returned by GitHub.".to_owned())?
    } else {
        doc
    };
    let tag = text(entry, "tag_name");
    let version = parse_tag_version(&tag)?;

    let mut zip: Option<(String, String, Option<String>)> = None;
    let mut integrity_url = None;
    for asset in entry
        .get("assets")
        .and_then(Value::as_array)
        .ok_or_else(|| "Release JSON has no assets list.".to_owned())?
    {
        let name = text(asset, "name");
        let url = text(asset, "browser_download_url");
        if ends_with_ignore_case(&name, ".zip") {
            log(format!("[updater] selected asset: {name} url={url}"));
            let digest = asset.get("digest").and_then(Value::as_str).map(str::to_owned);
            zip = Some((url, name, digest));
            continue;
        }
        if ends_with_ignore_case(&name, ".integrity.tsv") {
            log(format!("[updater] selected integrity asset: {name} url={url}"));
            integrity_url = Some(url);
        }
    }
    let Some((zip_url, zip_name, zip_digest)) = zip.filter(|(url, _, _)| !url.is_empty()) else {
        return Err("No .zip asset on latest release.".to_owned());
    };
    Ok(LatestRelease {
        version,
        tag,
        zip_url,
        zip_name,
        zip_digest,
        integrity_url: integrity_url.filter(|u| !u.trim().is_empty()),
        body: text(entry, "body"),
    })
}

pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn integrity_sha(text: &str, zip_name: &str) -> Option<String> {
    if text.trim().is_empty() || zip_name.trim().is_empty() {
        return None;
    }
    for raw in text.split('\n') {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() != 3 || !parts[2].trim().eq_ignore_ascii_case(zip_name) {
            continue;
        }
        let sha = parts[0].trim();
        return is_sha256_hex(sha).then(|| sha.to_ascii_uppercase());
    }
    None
}

pub fn asset_digest_sha(digest: Option<&str>) -> Option<String> {
    let trimmed = digest?.trim();
    let prefix = "sha256:";
    if trimmed.len() < prefix.len() || !trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let sha = trimmed[prefix.len()..].trim();
    is_sha256_hex(sha).then(|| sha.to_ascii_uppercase())
}

pub fn legacy_body_sha(body: &str) -> Option<String> {
    let caps = SHA256_LINE.as_ref()?.captures(body)?;
    Some(caps.get(1)?.as_str().to_ascii_uppercase())
}

pub fn expected_sha(
    release: &LatestRelease,
    fetch_integrity: impl FnOnce(&str) -> Result<String, String>,
    log: &mut dyn FnMut(String),
) -> Option<String> {
    if let Some(url) = release.integrity_url.as_deref() {
        match fetch_integrity(url) {
            Ok(text) => {
                if let Some(sha) = integrity_sha(&text, &release.zip_name) {
                    return Some(sha);
                }
            }
            Err(e) => log(format!("[updater] integrity asset read failed: {e}")),
        }
    }
    asset_digest_sha(release.zip_digest.as_deref()).or_else(|| legacy_body_sha(&release.body))
}

pub fn include_prereleases(settings_json: Option<&str>) -> bool {
    settings_json
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|doc| doc.get("maintenance")?.get("include_prereleases").cloned())
        == Some(Value::Bool(true))
}

pub fn safe_file_token(tag: &str) -> String {
    tag.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
