use std::collections::BTreeSet;
use std::net::Ipv6Addr;

use url::{Host, Url};

const WILDCARD_PREFIX: &str = "*.";
const SUFFIX_PREFIX: &str = ".";
const MAX_HOST_CHARS: usize = 253;
const MAX_LABEL_CHARS: usize = 63;
const MAX_ENTRY_CHARS: usize = WILDCARD_PREFIX.len() + MAX_HOST_CHARS + 1;
pub const MAX_ENTRIES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    Exact(String),
    Subdomains(String),
}

impl Rule {
    fn parse(entry: &str) -> Option<Self> {
        let entry = entry.trim().to_ascii_lowercase();
        if let Some(suffix) = entry
            .strip_prefix(WILDCARD_PREFIX)
            .or_else(|| entry.strip_prefix(SUFFIX_PREFIX))
        {
            return match parse_entry_host(suffix)? {
                CanonicalHost::Domain(domain) if domain.contains('.') => {
                    Some(Self::Subdomains(domain))
                }
                CanonicalHost::Domain(_) | CanonicalHost::Address(_) => None,
            };
        }
        parse_entry_host(&entry).map(|host| Self::Exact(host.into_name()))
    }

    fn matches(&self, host: &CanonicalHost) -> bool {
        match (self, host) {
            (Self::Exact(exact), host) => exact == host.name(),
            (Self::Subdomains(suffix), CanonicalHost::Domain(domain)) => domain
                .strip_suffix(suffix.as_str())
                .is_some_and(|head| head.len() > 1 && head.ends_with('.')),
            (Self::Subdomains(_), CanonicalHost::Address(_)) => false,
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Exact(exact) => exact.clone(),
            Self::Subdomains(suffix) => format!("{WILDCARD_PREFIX}{suffix}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CanonicalHost {
    Domain(String),
    Address(String),
}

impl CanonicalHost {
    fn from_url_host(host: &Host<&str>) -> Option<Self> {
        match host {
            Host::Domain(domain) => {
                let lower = domain.to_ascii_lowercase();
                let trimmed = lower.strip_suffix('.').unwrap_or(&lower);
                is_valid_domain(trimmed).then(|| Self::Domain(trimmed.to_owned()))
            }
            Host::Ipv4(address) => Some(Self::Address(address.to_string())),
            Host::Ipv6(address) => Some(Self::Address(address.to_string())),
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Domain(name) | Self::Address(name) => name,
        }
    }

    fn into_name(self) -> String {
        match self {
            Self::Domain(name) | Self::Address(name) => name,
        }
    }
}

fn is_valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= MAX_HOST_CHARS
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL_CHARS
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
}

fn parse_entry_host(entry: &str) -> Option<CanonicalHost> {
    if entry.is_empty() {
        return None;
    }
    if let Ok(address) = entry.parse::<Ipv6Addr>() {
        return Some(CanonicalHost::Address(address.to_string()));
    }
    let url = if entry.contains("://") {
        Url::parse(entry).ok()?
    } else if entry.contains(['/', '?', '#', '@', '\\']) {
        return None;
    } else {
        Url::parse(&format!("http://{entry}/")).ok()?
    };
    CanonicalHost::from_url_host(&url.host()?)
}

fn url_host(raw: &str) -> Option<CanonicalHost> {
    let url = Url::parse(raw.trim()).ok()?;
    CanonicalHost::from_url_host(&url.host()?)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeFallbackHosts {
    rules: Vec<Rule>,
    entries: Vec<String>,
}

impl NativeFallbackHosts {
    pub fn new<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut rules = BTreeSet::new();
        'entries: for entry in entries {
            for item in entry
                .as_ref()
                .split([',', ';'])
                .flat_map(str::split_whitespace)
            {
                if rules.len() == MAX_ENTRIES {
                    break 'entries;
                }
                if item.chars().nth(MAX_ENTRY_CHARS).is_some() {
                    continue;
                }
                if let Some(rule) = Rule::parse(item) {
                    rules.insert(rule);
                }
            }
        }
        let rules: Vec<Rule> = rules.into_iter().collect();
        let entries: BTreeSet<String> = rules.iter().map(Rule::display).collect();
        Self {
            rules,
            entries: entries.into_iter().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn matches_url(&self, raw: &str) -> bool {
        !self.is_empty()
            && url_host(raw).is_some_and(|host| self.rules.iter().any(|rule| rule.matches(&host)))
    }
}
