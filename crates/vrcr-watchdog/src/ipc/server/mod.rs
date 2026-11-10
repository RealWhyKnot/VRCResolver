use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use tokio::io::{AsyncRead, AsyncWrite};
use vrcr_shared::budget::retry;
use vrcr_shared::console::{Component, Outcome, ResolveStatus, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::logutil::{bare_host, sanitize_for_console};
use vrcr_shared::profile;
use vrcr_shared::wire::{
    self, DEFAULT_MAX_HEIGHT, HIGH_QUALITY_MAX_HEIGHT, Player, ResolveRequest, ResolveResponse,
    WrapperEventNotify, action, fallback, feedback, og_reason,
};

use super::framing::{self, cid_suffix};
use super::{Feedback, Mesh, MeshResult, caller_opted_into_v2};
use crate::health::{self, HealthGate, Transition};
use crate::local::{self, Ask, Step};
use crate::og_hint::{self, OgFallbackHint};
use crate::resolve_cache::{Key, ResolveCache};
use crate::stats::Stats;

pub const PER_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const WRAPPER_BUDGET_MINIMUM_MS: i32 = 1_000;
pub const FULL_WINDOW_BUDGET_MS: i32 = 20_000;
const WRAPPER_BUDGET_CEILING_MS: i32 = 90_000;
const WRAPPER_BUDGET_SAFETY_MARGIN_MS: i32 = 500;
pub const OG_FAILED_DETAIL_UNKNOWN: &str = "unknown";
pub const OG_FAILED_DETAILS: [&str; 5] = [
    "cf_403",
    "rate_limited",
    "sign_in_required",
    "content_not_found",
    OG_FAILED_DETAIL_UNKNOWN,
];

pub fn effective_budget_ms(wrapper_deadline_ms: i32) -> i32 {
    wrapper_deadline_ms
        .saturating_sub(WRAPPER_BUDGET_SAFETY_MARGIN_MS)
        .clamp(WRAPPER_BUDGET_MINIMUM_MS, WRAPPER_BUDGET_CEILING_MS)
}

pub fn is_healthy_outcome(
    fail_reason: Option<&str>,
    outcome: &str,
    server_reason: Option<&str>,
    budget_ms: i32,
) -> bool {
    let server_fallback = outcome == action::FALLBACK_NATIVE;
    let deadline_hit = fail_reason == Some(fallback::CLIENT_DEADLINE_EXCEEDED)
        || (server_fallback && server_reason == Some(fallback::CLIENT_DEADLINE_EXCEEDED));
    if deadline_hit {
        return budget_ms < FULL_WINDOW_BUDGET_MS;
    }
    fail_reason.is_none()
        && !(server_fallback
            && matches!(
                server_reason,
                Some(fallback::SERVER_UNREACHABLE | fallback::INTERNAL_ERROR)
            ))
}

pub fn bypasses_health_gate(req: &ResolveRequest, hint: Option<&OgFallbackHint>) -> bool {
    req.skip_native_hint == Some(true) || hint.is_some_and(|h| h.is_og_blocked(&req.url))
}

pub fn is_site_wide_og_block(reason: Option<&str>) -> bool {
    matches!(reason, Some("sign_in_required" | "cf_403" | "rate_limited"))
}

pub fn og_failed_detail_for(reason: Option<&str>) -> &'static str {
    reason
        .and_then(|r| OG_FAILED_DETAILS.iter().find(|d| **d == r).copied())
        .unwrap_or(OG_FAILED_DETAIL_UNKNOWN)
}

type Flag = Arc<dyn Fn() -> bool + Send + Sync>;
type Codecs = Arc<dyn Fn() -> Option<Vec<String>> + Send + Sync>;
type FallbackSink = Arc<dyn Fn(&ResolveRequest, &str) + Send + Sync>;

pub struct IpcBuilder<M: Mesh> {
    ipc: Ipc<M>,
}

impl<M: Mesh> IpcBuilder<M> {
    pub fn cache(mut self, cache: Arc<ResolveCache>) -> Self {
        self.ipc.cache = Some(cache);
        self
    }

    pub fn og_hint(mut self, hint: Arc<OgFallbackHint>) -> Self {
        self.ipc.hint = Some(hint);
        self
    }

    pub fn health(mut self, health: Arc<HealthGate>) -> Self {
        self.ipc.health = Some(health);
        self
    }

    pub fn stats(mut self, stats: Arc<Stats>) -> Self {
        self.ipc.stats = stats;
        self
    }

    pub fn high_quality(mut self, f: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.ipc.high_quality = Arc::new(f);
        self
    }

    pub fn verified_codecs(
        mut self,
        f: impl Fn() -> Option<Vec<String>> + Send + Sync + 'static,
    ) -> Self {
        self.ipc.verified_codecs = Arc::new(f);
        self
    }

    pub fn report_fallback(
        mut self,
        f: impl Fn(&ResolveRequest, &str) + Send + Sync + 'static,
    ) -> Self {
        self.ipc.report_fallback = Arc::new(f);
        self
    }

    pub fn local(mut self, step: Option<Arc<dyn Step>>) -> Self {
        self.ipc.local = step;
        self
    }

    pub fn build(self) -> Arc<Ipc<M>> {
        Arc::new(self.ipc)
    }
}

pub struct Ipc<M: Mesh> {
    mesh: Arc<M>,
    ux: Ux,
    log: Log,
    cache: Option<Arc<ResolveCache>>,
    hint: Option<Arc<OgFallbackHint>>,
    health: Option<Arc<HealthGate>>,
    stats: Arc<Stats>,
    high_quality: Flag,
    verified_codecs: Codecs,
    report_fallback: FallbackSink,
    local: Option<Arc<dyn Step>>,
}

enum Failure {
    Deadline,
    Internal,
}

impl Failure {
    fn reason(&self) -> &'static str {
        match self {
            Failure::Deadline => fallback::CLIENT_DEADLINE_EXCEEDED,
            Failure::Internal => fallback::INTERNAL_ERROR,
        }
    }
}

impl<M: Mesh> Ipc<M> {
    pub fn builder(mesh: Arc<M>, ux: Ux, log: Log) -> IpcBuilder<M> {
        IpcBuilder {
            ipc: Ipc {
                mesh,
                ux,
                log,
                cache: None,
                hint: None,
                health: None,
                stats: Arc::new(Stats::default()),
                high_quality: Arc::new(|| false),
                verified_codecs: Arc::new(|| None),
                report_fallback: Arc::new(|_, _| {}),
                local: None,
            },
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub async fn handle<S: AsyncRead + AsyncWrite + Unpin>(&self, pipe: &mut S) {
        let started = Instant::now();
        let line = match tokio::time::timeout(PER_REQUEST_TIMEOUT, framing::read_line(pipe)).await {
            Ok(Ok(line)) => line,
            Ok(Err(e)) => {
                self.ux.warn(
                    Component::Ipc,
                    &format!("connection error id=: {:?}", e.kind()),
                );
                return;
            }
            Err(_) => {
                self.ux.warn(
                    Component::Ipc,
                    "connection error id=: request read timed out",
                );
                return;
            }
        };
        if line.truncated {
            self.ux.warn(
                Component::Ipc,
                &format!(
                    "rejecting request: payload exceeded {} bytes without a newline terminator",
                    framing::MAX_REQUEST_BYTES
                ),
            );
            framing::write_fallback(pipe, "", fallback::INTERNAL_ERROR).await;
            return;
        }
        let text = line.text.unwrap_or_default();
        if !text.trim().is_empty() && framing::looks_like_wrapper_event(&text) {
            match serde_json::from_str::<WrapperEventNotify>(&text) {
                Ok(n) => self.handle_wrapper_event(&n),
                Err(e) => self.log.line(&format!(
                    "[wrapper][warn] wrapper event parse failed: {:?} line={} column={} bytes={}",
                    e.classify(),
                    e.line(),
                    e.column(),
                    text.len()
                )),
            }
            return;
        }
        let parsed = if text.trim().is_empty() {
            Ok(None)
        } else {
            serde_json::from_str::<Option<ResolveRequest>>(&text)
        };
        let req = match parsed {
            Ok(Some(req)) if !req.url.is_empty() => req,
            other => {
                let msg = match other {
                    Err(e) => format!(
                        "request parse failed: {:?} line={} column={} bytes={}",
                        e.classify(),
                        e.line(),
                        e.column(),
                        text.len()
                    ),
                    Ok(Some(_)) => "request missing url".to_owned(),
                    Ok(None) => "empty request received".to_owned(),
                };
                self.ux.warn(Component::Ipc, &msg);
                framing::write_fallback(pipe, "", fallback::INTERNAL_ERROR).await;
                return;
            }
        };
        self.resolve(pipe, req, started).await;
    }

    pub fn handle_wrapper_event(&self, n: &WrapperEventNotify) {
        if n.action == action::WRAPPER_OG_FAILED {
            self.og_failed(n);
        } else {
            self.og_fallback(n);
        }
    }

    fn og_fallback(&self, n: &WrapperEventNotify) {
        let url = n.url.as_deref().unwrap_or_default();
        let host = if url.is_empty() {
            "<no-url>".to_owned()
        } else {
            bare_host(url)
        };
        let reason = sanitize_for_console(n.reason.as_deref().unwrap_or("?"), 32);
        if let Some(msg) = n.public_message.as_deref().filter(|m| !m.is_empty()) {
            self.ux
                .warn(Component::Wrapper, &sanitize_for_console(msg, 160));
        }
        self.ux
            .wrapper_fallback(&host, &reason, u64::try_from(n.elapsed_ms).unwrap_or(0));
        self.log.line(&format!(
            "[wrapper] og_fallback_notify rid={} host={host} reason={reason} elapsed_ms={}",
            sanitize_for_console(n.rid.as_deref().unwrap_or("?"), 16),
            n.elapsed_ms
        ));
        if !url.is_empty()
            && matches!(
                n.reason.as_deref(),
                Some(og_reason::RESOLVED_URL_REJECTED | og_reason::AVPRO_INCOMPATIBLE)
            )
        {
            self.mesh.send_playback_feedback(Feedback {
                url: url.to_owned(),
                kind: feedback::RESOLVED_REJECTED,
                ms_since_open: 0,
                delivered_height: None,
                detail: n.reason.clone(),
                correlation_id: n.rid.clone(),
            });
        }
    }

    fn og_failed(&self, n: &WrapperEventNotify) {
        let url = n.url.as_deref().unwrap_or_default();
        let host = if url.is_empty() {
            "<no-url>".to_owned()
        } else {
            bare_host(url)
        };
        let reason = sanitize_for_console(n.reason.as_deref().unwrap_or("?"), 32);
        let mut evicted = 0;
        let mut hint_cleared = false;
        let mut newly_blocked = false;
        if !url.is_empty() {
            evicted = self
                .cache
                .as_deref()
                .map(|c| c.evict_by_url(url))
                .unwrap_or(0);
            if let Some(h) = self.hint.as_deref() {
                if is_site_wide_og_block(n.reason.as_deref()) {
                    newly_blocked = h.record_og_blocked(url);
                }
                hint_cleared = h.try_clear(url);
            }
        }
        let suffix = match reason.as_str() {
            "content_not_found" => " (video unavailable upstream)",
            "cf_403" => " (403 blocked)",
            "rate_limited" => " (rate limited)",
            "sign_in_required" => " (auth gate)",
            _ => "",
        };
        self.ux.warn(
            Component::Wrapper,
            &format!(
                "!! og also failed {host} reason={reason} exit={}{suffix}",
                n.exit_code
            ),
        );
        if newly_blocked {
            let site = og_hint::site_key(url);
            self.ux.warn(
                Component::Wrapper,
                &format!(
                    "VRChat's own resolver is blocked on {site}, so {site} videos stay on our resolver for {} min",
                    og_hint::OG_BLOCKED_TTL.as_secs() / 60
                ),
            );
        }
        self.log.line(&format!(
            "[wrapper] wrapper_og_failed rid={} host={host} reason={reason} exit={} elapsed_ms={} evicted={evicted} hint_cleared={}",
            sanitize_for_console(n.rid.as_deref().unwrap_or("?"), 16),
            n.exit_code,
            n.elapsed_ms,
            if hint_cleared { "True" } else { "False" }
        ));
        if !url.is_empty() {
            self.mesh.send_playback_feedback(Feedback {
                url: url.to_owned(),
                kind: feedback::OG_FAILED,
                ms_since_open: 0,
                delivered_height: None,
                detail: Some(og_failed_detail_for(n.reason.as_deref()).to_owned()),
                correlation_id: n.rid.clone(),
            });
        }
    }

    #[cfg(windows)]
    pub fn serve(self: &Arc<Self>, pipe_name: &str) -> tokio::task::JoinHandle<()> {
        let me = std::sync::Arc::clone(self);
        let name = pipe_name.to_owned();
        tokio::spawn(async move { me.accept_loop(name).await })
    }

    #[cfg(windows)]
    async fn accept_loop(self: Arc<Self>, pipe_name: String) {
        let sddl = match super::pipe::current_user_sid() {
            Ok(sid) => super::pipe::pipe_sddl(&sid),
            Err(e) => {
                self.ux.error(
                    Component::Ipc,
                    &format!("could not resolve current user SID: {e}"),
                );
                return;
            }
        };
        let path = format!(r"\\.\pipe\{pipe_name}");
        let mut clients = tokio::task::JoinSet::new();
        let mut last_capacity_warning = None;
        loop {
            let mut server = match super::pipe::create(&path, &sddl) {
                Ok(s) => s,
                Err(e) => {
                    self.ux.warn(
                        Component::Ipc,
                        &format!("could not create pipe instance: {e}"),
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            if let Err(e) = server.connect().await {
                self.ux.warn(Component::Ipc, &format!("accept failed: {e}"));
                continue;
            }
            while clients.try_join_next().is_some() {}
            if clients.len() >= 128 {
                let result = server.disconnect();
                if should_warn_at_capacity(&mut last_capacity_warning, Instant::now()) {
                    let detail = match result {
                        Ok(()) => "connection closed".to_owned(),
                        Err(error) => format!("disconnect failed: {:?}", error.kind()),
                    };
                    self.log
                        .line(&format!("[ipc][warn] client limit reached (128); {detail}"));
                }
                continue;
            }
            let me = Arc::clone(&self);
            clients.spawn(async move {
                me.handle(&mut server).await;
                let _ = server.disconnect();
            });
        }
    }
}

#[cfg(windows)]
fn should_warn_at_capacity(last_warning: &mut Option<Instant>, now: Instant) -> bool {
    if last_warning
        .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(60))
    {
        return false;
    }
    *last_warning = Some(now);
    true
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn client_limit_warning_resumes_after_one_minute() {
        let now = Instant::now();
        let mut last = None;
        assert!(should_warn_at_capacity(&mut last, now));
        assert!(!should_warn_at_capacity(&mut last, now));
        assert!(!should_warn_at_capacity(
            &mut last,
            now + Duration::from_secs(59)
        ));
        assert!(should_warn_at_capacity(
            &mut last,
            now + Duration::from_secs(60)
        ));
    }
}

fn remaining_ms(deadline: Instant) -> i64 {
    i64::try_from(
        deadline
            .saturating_duration_since(Instant::now())
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

mod resolution;
