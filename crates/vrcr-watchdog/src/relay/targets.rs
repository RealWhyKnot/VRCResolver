use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use url::Url;
use vrcr_shared::b64;
use vrcr_shared::first_party::FirstPartyPolicy;

use super::security;

const RELATIVE_BASE_SOFT_CAP: usize = 256;
const RELATIVE_BASE_HARD_CAP: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Target,
    Relative,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Target => "target",
            TargetKind::Relative => "relative",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTarget {
    pub url: String,
    pub kind: TargetKind,
}

#[derive(Default)]
pub struct TargetResolver {
    bases: Mutex<Bases>,
}

#[derive(Default)]
struct Bases {
    by_prefix: HashMap<String, (String, String)>,
    order: VecDeque<String>,
}

impl TargetResolver {
    pub fn new() -> TargetResolver {
        TargetResolver::default()
    }

    pub fn resolve(
        &self,
        policy: &FirstPartyPolicy,
        path: &str,
        query: &str,
        target_param: Option<&str>,
    ) -> Option<RelayTarget> {
        if let Some(param) = target_param.filter(|p| !p.is_empty()) {
            let url = b64::decode(param).unwrap_or_default();
            if let Ok(parsed) = security::check_target_url(policy, &url) {
                self.remember_base(path, &parsed);
            }
            return Some(RelayTarget {
                url,
                kind: TargetKind::Target,
            });
        }
        let (prefix, base) = self.best_base(path)?;
        let url = resolve_relative_target(path, query, &prefix, &base)?;
        Some(RelayTarget {
            url,
            kind: TargetKind::Relative,
        })
    }

    pub fn remembered(&self) -> usize {
        self.bases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .by_prefix
            .len()
    }

    fn remember_base(&self, local_path: &str, target: &Url) {
        let Ok(base) = target.join(".") else {
            return;
        };
        let prefix = local_prefix_for_path(local_path);
        let key = prefix.to_ascii_lowercase();
        let mut bases = self.bases.lock().unwrap_or_else(|e| e.into_inner());
        if !bases.by_prefix.contains_key(&key) {
            bases.order.push_back(key.clone());
        }
        bases.by_prefix.insert(key, (prefix, base.to_string()));
        while bases.by_prefix.len() > RELATIVE_BASE_HARD_CAP {
            let Some(old) = bases.order.pop_front() else {
                break;
            };
            if bases.by_prefix.len() <= RELATIVE_BASE_SOFT_CAP {
                break;
            }
            bases.by_prefix.remove(&old);
        }
    }

    fn best_base(&self, path: &str) -> Option<(String, String)> {
        let bases = self.bases.lock().unwrap_or_else(|e| e.into_inner());
        bases
            .by_prefix
            .values()
            .filter(|(prefix, _)| starts_with_ignore_case(path, prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .cloned()
    }
}

pub fn local_prefix_for_path(path: &str) -> String {
    if !is_play_path(path) || path.eq_ignore_ascii_case("/play") {
        return "/play/".to_owned();
    }
    if path.ends_with('/') {
        return path.to_owned();
    }
    match path.rfind('/') {
        Some(slash) => path[..=slash].to_owned(),
        None => "/play/".to_owned(),
    }
}

pub fn resolve_relative_target(local_path: &str, query: &str, prefix: &str, base: &str) -> Option<String> {
    if local_path.is_empty() || prefix.is_empty() || !starts_with_ignore_case(local_path, prefix) {
        return None;
    }
    let base = Url::parse(base).ok()?;
    let relative = &local_path[prefix.len()..];
    if relative.is_empty() {
        return None;
    }
    let joined = format!("{relative}{query}");
    base.join(&joined).ok().map(|u| u.to_string())
}

pub fn is_play_path(path: &str) -> bool {
    path.eq_ignore_ascii_case("/play") || starts_with_ignore_case(path, "/play/")
}

pub fn encode_target_param(url: &str) -> String {
    b64::encode(url)
}

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}
