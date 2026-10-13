use std::cmp::Ordering;

use serde_json::Value;

pub const REPO: &str = "RealWhyKnot/VRCResolver";
pub const STABLE_LATEST_URL: &str = "https://api.github.com/repos/RealWhyKnot/VRCResolver/releases/latest";
pub const ANY_RELEASES_URL: &str =
    "https://api.github.com/repos/RealWhyKnot/VRCResolver/releases?per_page=10";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version([i64; 4]);

impl Version {
    pub const ZERO: Version = Version([0, 0, 0, 0]);

    pub fn parse(text: &str) -> Option<Version> {
        let parts: Vec<&str> = text.split('.').collect();
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        let mut v = [-1i64; 4];
        for (slot, part) in v.iter_mut().zip(&parts) {
            let p = part.trim();
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: i64 = p.parse().ok()?;
            if n > i64::from(i32::MAX) {
                return None;
            }
            *slot = n;
        }
        Some(Version(v))
    }

    pub fn from_tag(tag: &str) -> Option<Version> {
        let numeric = tag.trim_start_matches(['v', 'V']);
        let numeric = numeric.split_once('-').map_or(numeric, |(head, _)| head);
        Version::parse(numeric)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<String> = self
            .0
            .iter()
            .take_while(|n| **n >= 0)
            .map(i64::to_string)
            .collect();
        f.write_str(&parts.join("."))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

pub fn is_prerelease(entry: &Value) -> bool {
    entry.get("prerelease") == Some(&Value::Bool(true))
}

pub fn pick_highest_entry(list: &Value) -> Option<&Value> {
    let mut best: Option<(Version, &Value)> = None;
    for entry in list.as_array()? {
        if !entry.is_object() {
            continue;
        }
        let Some(version) = entry
            .get("tag_name")
            .and_then(Value::as_str)
            .and_then(Version::from_tag)
        else {
            continue;
        };
        if let Some((best_version, best_entry)) = &best {
            match version.cmp(best_version) {
                Ordering::Less => continue,
                Ordering::Equal if is_prerelease(entry) || !is_prerelease(best_entry) => continue,
                _ => {}
            }
        }
        best = Some((version, entry));
    }
    best.map(|(_, e)| e)
}
