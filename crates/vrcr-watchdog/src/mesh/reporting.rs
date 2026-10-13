use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex_lite::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};

use vrcr_shared::console::{Component, Ux};
use vrcr_shared::legacy::env_with_fallback;
use vrcr_shared::wire::{Player, ResolveRequest, fallback};

pub const REPORT_URL: &str = "https://vrcresolver.com/api/report";
pub const MAX_PER_SESSION: u32 = 20;
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);
pub const POST_TIMEOUT: Duration = Duration::from_secs(20);
pub const MAX_SUMMARY_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportPayload {
    pub app_version: String,
    pub failure_kind: String,
    pub url_domain: String,
    pub player: String,
    pub stream_type: String,
    pub url_path_hash_short: String,
    pub video_id_hash_short: String,
    pub error_summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub user_name: Option<String>,
    pub user_profile: Option<String>,
    pub machine_name: Option<String>,
}

impl Machine {
    pub fn from_env() -> Machine {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Machine {
            user_name: var("USERNAME").or_else(|| var("USER")),
            user_profile: var("USERPROFILE"),
            machine_name: var("COMPUTERNAME").or_else(|| var("HOSTNAME")),
        }
    }
}

pub type Post = Arc<dyn Fn(&ReportPayload) -> Result<u16, String> + Send + Sync>;

struct Patterns {
    ipv4: Regex,
    windows_path: Regex,
    unix_path: Regex,
    long_token: Regex,
    youtube_id: Regex,
}

impl Patterns {
    fn new() -> Patterns {
        let compile = |p: &str| Regex::new(p).unwrap_or_else(|e| panic!("bad built-in pattern {p}: {e}"));
        Patterns {
            ipv4: compile(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b"),
            windows_path: compile(r#"[A-Za-z]:\\[^\s"]+"#),
            unix_path: compile(r#"/(?:home|Users|root)/[^\s"]+"#),
            long_token: compile(r"[A-Za-z0-9+/=_\-]{20,}"),
            youtube_id: compile(r"(?:v=|/embed/|/v/|/live/|youtu\.be/)([A-Za-z0-9_-]{11})"),
        }
    }
}

#[derive(Default)]
struct Budget {
    sent: u32,
    last: Option<Instant>,
}

pub struct Reporter {
    enabled: bool,
    app_version: String,
    ux: Ux,
    machine: Machine,
    patterns: Patterns,
    budget: Mutex<Budget>,
    post: Post,
}

pub fn failure_kind(reason: &str) -> Option<&'static str> {
    match reason {
        fallback::ALL_CONFIGS_FAILED => Some("AllStrategiesFailed"),
        fallback::EXTRACTOR_UNSUPPORTED | fallback::INTERNAL_ERROR | fallback::UNITY_UNSUPPORTED_FORMAT => {
            Some("Unknown")
        }
        fallback::WARP_DOWN => Some("NetworkError"),
        _ => None,
    }
}

pub fn hash_short(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    hex::encode(&Sha256::digest(text.as_bytes())[..6])
}

pub fn extract_domain(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(str::to_owned).unwrap_or(host))
}

pub fn path_and_query(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return String::new();
    };
    match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_owned(),
    }
}

pub fn replace_ignore_ascii_case(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let hay = text.to_ascii_lowercase();
    let pin = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = hay[at..].find(&pin) {
        let start = at + found;
        out.push_str(&text[at..start]);
        out.push_str(with);
        at = start + pin.len();
    }
    out.push_str(&text[at..]);
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub fn http_post(app_version: &str) -> Post {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(POST_TIMEOUT))
        .user_agent(format!("VRCResolver-Reporter/{app_version}"))
        .http_status_as_error(false)
        .build()
        .into();
    Arc::new(move |payload: &ReportPayload| {
        agent
            .post(REPORT_URL)
            .send_json(payload)
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    })
}

impl Reporter {
    pub fn new(enabled: bool, app_version: &str, ux: Ux, machine: Machine, post: Post) -> Reporter {
        Reporter {
            enabled,
            app_version: app_version.to_owned(),
            ux,
            machine,
            patterns: Patterns::new(),
            budget: Mutex::new(Budget::default()),
            post,
        }
    }

    pub fn from_env(app_version: &str, ux: Ux) -> Reporter {
        let enabled = env_with_fallback("ANONYMOUS_REPORTING").as_deref() == Some("1");
        if enabled {
            ux.write(
                Component::Report,
                "anonymous failure reporting ON (VRCRESOLVER_ANONYMOUS_REPORTING=1)",
            );
        }
        Reporter::new(
            enabled,
            app_version,
            ux,
            Machine::from_env(),
            http_post(app_version),
        )
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn report_fallback(
        self: &Arc<Self>,
        req: &ResolveRequest,
        reason: &str,
        error_summary: Option<&str>,
    ) {
        if !self.enabled || req.url.is_empty() {
            return;
        }
        let Some(kind) = failure_kind(reason) else {
            return;
        };
        if !self.take_slot(Instant::now()) {
            return;
        }
        let Some(payload) = self.build(req, kind, error_summary) else {
            return;
        };
        let me = Arc::clone(self);
        let job = move || me.send(&payload);
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn_blocking(job);
            }
            Err(_) => {
                std::thread::spawn(job);
            }
        }
    }

    pub fn take_slot(&self, now: Instant) -> bool {
        let mut b = self.budget.lock().unwrap_or_else(|e| e.into_inner());
        if b.sent >= MAX_PER_SESSION {
            return false;
        }
        if b.last.is_some_and(|last| now.duration_since(last) < MIN_INTERVAL) {
            return false;
        }
        b.sent += 1;
        b.last = Some(now);
        true
    }

    pub fn send(&self, payload: &ReportPayload) {
        match (self.post)(payload) {
            Ok(code) if (200..300).contains(&code) => {}
            Ok(code) => self
                .ux
                .warn(Component::Report, &format!("server rejected report: {code}")),
            Err(e) => self.ux.warn(Component::Report, &format!("post failed: {e}")),
        }
    }

    pub fn build(
        &self,
        req: &ResolveRequest,
        kind: &str,
        error_summary: Option<&str>,
    ) -> Option<ReportPayload> {
        let domain = extract_domain(&req.url).filter(|d| !d.is_empty())?;
        let player = if req.player == Some(Player::Unity) {
            Player::Unity
        } else {
            Player::AvPro
        };
        Some(ReportPayload {
            app_version: self.app_version.clone(),
            failure_kind: kind.to_owned(),
            url_domain: domain,
            player: player.as_str().to_owned(),
            stream_type: "vod".to_owned(),
            url_path_hash_short: hash_short(&path_and_query(&req.url)),
            video_id_hash_short: hash_short(&self.youtube_id(&req.url).unwrap_or_default()),
            error_summary: truncate_chars(
                &self.sanitize(error_summary.unwrap_or_default(), Some(&req.url)),
                MAX_SUMMARY_CHARS,
            ),
        })
    }

    pub fn youtube_id(&self, url: &str) -> Option<String> {
        self.patterns
            .youtube_id
            .captures(url)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
    }

    pub fn sanitize(&self, text: &str, original_url: Option<&str>) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut s = text.to_owned();
        if let Some(user) = self.machine.user_name.as_deref().filter(|u| u.len() >= 3) {
            s = replace_ignore_ascii_case(&s, user, "<user>");
        }
        if let Some(profile) = self.machine.user_profile.as_deref() {
            s = replace_ignore_ascii_case(&s, profile, "%USERPROFILE%");
        }
        if let Some(url) = original_url.filter(|u| !u.is_empty()) {
            s = replace_ignore_ascii_case(&s, url, "<url>");
        }
        if let Some(machine) = self.machine.machine_name.as_deref() {
            s = replace_ignore_ascii_case(&s, machine, "<machine>");
        }
        let p = &self.patterns;
        let s = p.windows_path.replace_all(&s, "<path>");
        let s = p.unix_path.replace_all(&s, "<path>");
        let s = p.ipv4.replace_all(&s, "<ip>");
        p.long_token.replace_all(&s, "<token>").into_owned()
    }
}
