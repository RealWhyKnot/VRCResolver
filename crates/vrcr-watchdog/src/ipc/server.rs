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

    pub fn verified_codecs(mut self, f: impl Fn() -> Option<Vec<String>> + Send + Sync + 'static) -> Self {
        self.ipc.verified_codecs = Arc::new(f);
        self
    }

    pub fn report_fallback(mut self, f: impl Fn(&ResolveRequest, &str) + Send + Sync + 'static) -> Self {
        self.ipc.report_fallback = Arc::new(f);
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
                self.ux
                    .warn(Component::Ipc, &format!("connection error id=: {:?}", e.kind()));
                return;
            }
            Err(_) => {
                self.ux
                    .warn(Component::Ipc, "connection error id=: request read timed out");
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
                    "[wrapper][warn] wrapper event parse failed: {}",
                    sanitize_for_console(&e.to_string(), 160)
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
                        "request parse failed: {} preview={}",
                        sanitize_for_console(&e.to_string(), 160),
                        sanitize_for_console(&text, 80)
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

    async fn resolve<S: AsyncWrite + Unpin>(&self, pipe: &mut S, mut req: ResolveRequest, started: Instant) {
        let id = req.id.clone();
        let mut cid = req.correlation_id.clone();
        if req.action != action::RESOLVE {
            self.ux.warn(
                Component::Ipc,
                &format!(
                    "rejecting request id={id} action={} -- only \"resolve\" is accepted on this pipe",
                    sanitize_for_console(&req.action, 32)
                ),
            );
            framing::write_fallback(pipe, &id, fallback::INTERNAL_ERROR).await;
            return;
        }
        if !matches!(req.player, Some(Player::AvPro | Player::Unity)) {
            self.ux.warn(
                Component::Ipc,
                &format!(
                    "rejecting request id={id}{} player={} -- must be \"avpro\" or \"unity\" (case-sensitive)",
                    cid_suffix(cid.as_deref()),
                    req.player.map(Player::as_str).unwrap_or("<null>")
                ),
            );
            framing::write_fallback(pipe, &id, fallback::INTERNAL_ERROR).await;
            return;
        }

        let mut budget_ms = PER_REQUEST_TIMEOUT.as_millis() as i32;
        let mut deadline = started + PER_REQUEST_TIMEOUT;
        if let Some(wrapper_ms) = req.wrapper_deadline_ms {
            budget_ms = effective_budget_ms(wrapper_ms);
            deadline = Instant::now() + Duration::from_millis(budget_ms as u64);
            self.log.line(&format!(
                "[ipc] honoring wrapper_deadline_ms id={id} wrapper_deadline_ms={wrapper_ms} effective_ms={budget_ms}"
            ));
        }

        if req.player == Some(Player::AvPro) && req.accept_codecs.is_some() {
            let verified = (self.verified_codecs)();
            req.accept_codecs = Some(
                wire::build_accept_codecs(verified.as_deref())
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            );
        }
        if profile::apply_high_quality(&mut req, (self.high_quality)()) {
            self.log.line(&format!(
                "[ipc] high quality on; requesting up to {HIGH_QUALITY_MAX_HEIGHT}p id={id}"
            ));
        } else if profile::apply_default_quality_cap(&mut req) {
            self.log.line(&format!(
                "[ipc] capped VRChat's height request to {DEFAULT_MAX_HEIGHT}p id={id}"
            ));
        }

        let host = bare_host(&req.url);
        let via_lh_yt = framing::is_localhost_youtube_url(&req.url);
        let player_label = framing::player_label(&req);
        self.stats.record_resolve(via_lh_yt);
        let node_host = self.mesh.current_node_host();
        let hint = self.hint.as_deref();

        if req.skip_native_hint == Some(true) {
            if let Some(h) = hint {
                h.try_clear(&req.url);
            }
        } else if hint.is_some_and(|h| h.should_prefer_og(&req.url)) {
            framing::write_fallback(pipe, &id, og_reason::PRIOR_LOAD_FAILURE).await;
            self.ux.write(
                Component::Ipc,
                &format!(
                    "og-fallback (prior load_failure) id={id}{} host={host}",
                    cid_suffix(cid.as_deref())
                ),
            );
            return;
        }

        let mut is_probe = false;
        if let Some(gate) = self.health.as_deref()
            && !bypasses_health_gate(&req, hint)
        {
            let (paused, shift) = gate.should_short_circuit(self.mesh.is_connected());
            if shift == Transition::Closed {
                self.ux
                    .success(Component::Ipc, "resolver restored -- mesh resolving re-enabled");
            }
            if paused {
                framing::write_fallback(pipe, &id, og_reason::RESOLVER_UNHEALTHY).await;
                self.ux.write(
                    Component::Ipc,
                    &format!(
                        "og-fallback (resolver paused) id={id}{} host={host}",
                        cid_suffix(cid.as_deref())
                    ),
                );
                return;
            }
            is_probe = gate.awaiting_probe_verdict();
        }

        let key_player = req.player.map(Player::as_str);
        let mut outcome = "?".to_owned();
        let mut server_reason: Option<String> = None;
        let mut via_cache = false;
        let mut failure: Option<Failure> = None;

        let cached = if is_probe {
            None
        } else {
            self.cache.as_deref().and_then(|c| {
                c.lookup(
                    &Key {
                        node: &node_host,
                        url: &req.url,
                        player: key_player,
                        format_arg: req.vrchat_format_arg.as_deref(),
                        max_height: req.max_height,
                    },
                    &req.id,
                )
            })
        };

        if let Some(hit) = cached {
            if framing::write_frame(pipe, &hit.frame).await.is_err() {
                self.log
                    .line(&format!("[ipc] wrapper left before the reply id={id}"));
            }
            outcome = hit.action;
            server_reason = hit.reason;
            via_cache = true;
            self.stats.record_cache_hit();
            self.log.line(&format!(
                "[resolve-cache] hit id={id} host={host} bytes={}",
                hit.frame.len()
            ));
            self.mesh.send_playback_feedback(Feedback {
                url: req.url.clone(),
                kind: feedback::CACHE_PLAY,
                ms_since_open: 0,
                delivered_height: None,
                detail: None,
                correlation_id: Some(
                    cid.clone()
                        .filter(|c| !c.is_empty())
                        .unwrap_or_else(|| id.clone()),
                ),
            });
        } else {
            if req.correlation_id.as_deref().unwrap_or_default().is_empty() && caller_opted_into_v2(&req) {
                req.correlation_id = Some(req.id.clone());
                cid = req.correlation_id.clone();
            }
            match self
                .resolve_with_retries(&mut req, deadline, cid.as_deref())
                .await
            {
                Ok(result) => {
                    if framing::write_frame(pipe, &result.frame).await.is_err() {
                        self.log
                            .line(&format!("[ipc] wrapper left before the reply id={id}"));
                    }
                    outcome = result.action;
                    server_reason = result.reason;
                    if outcome == action::RESOLVED && !node_host.is_empty() {
                        self.store_in_cache(&req, &node_host, &result.frame, &id, &host);
                    }
                }
                Err(f) => failure = Some(f),
            }
        }

        let fail_reason = failure.as_ref().map(Failure::reason);
        if let Some(reason) = fail_reason {
            outcome = format!("{}/{reason}", action::FALLBACK_NATIVE);
            framing::write_fallback(pipe, &id, reason).await;
            (self.report_fallback)(&req, reason);
        } else if let Some(reason) = outcome
            .strip_prefix(action::FALLBACK_NATIVE)
            .and_then(|r| r.strip_prefix('/'))
            .filter(|r| !r.is_empty())
        {
            (self.report_fallback)(&req, reason);
        }

        let (status, reason_for_line) = if outcome == action::RESOLVED {
            (ResolveStatus::Resolved, None)
        } else if let Some(r) = fail_reason {
            (ResolveStatus::Failed, Some(r.to_owned()))
        } else if outcome == action::FALLBACK_NATIVE {
            (
                ResolveStatus::Fallback,
                Some(
                    server_reason
                        .clone()
                        .filter(|r| !r.is_empty())
                        .unwrap_or_else(|| "unspecified".to_owned()),
                ),
            )
        } else {
            (ResolveStatus::Unexpected, Some(outcome.clone()))
        };
        let elapsed = started.elapsed();
        self.ux.resolve_outcome(&Outcome {
            host: &host,
            player: &player_label,
            status,
            via_cache,
            via_lh_yt,
            elapsed,
            reason: reason_for_line.as_deref(),
        });

        if let Some(gate) = self.health.as_deref()
            && !via_cache
        {
            let healthy = is_healthy_outcome(fail_reason, &outcome, server_reason.as_deref(), budget_ms);
            match gate.record_resolve_outcome(healthy, outcome == action::RESOLVED) {
                Transition::Opened => self.ux.warn(
                    Component::Ipc,
                    &format!(
                        "resolver paused -- {} resolves failed in a row; VRChat's own resolver takes over while the connection recovers",
                        health::OPEN_THRESHOLD
                    ),
                ),
                Transition::Closed => self.ux.success(Component::Ipc, "resolver restored -- mesh resolving re-enabled"),
                Transition::None => {}
            }
        }

        self.log.line(&format!(
            "[ipc] resolve_dispatch_complete id={id}{} action={} reason={} player={}{} elapsed_ms={}",
            cid_suffix(cid.as_deref()),
            sanitize_for_console(&outcome, 48),
            sanitize_for_console(server_reason.as_deref().or(fail_reason).unwrap_or_default(), 48),
            key_player.unwrap_or("unknown"),
            if via_cache { " via=cache" } else { "" },
            elapsed.as_millis()
        ));
    }

    async fn resolve_with_retries(
        &self,
        req: &mut ResolveRequest,
        deadline: Instant,
        cid: Option<&str>,
    ) -> Result<MeshResult, Failure> {
        let id = req.id.clone();
        let mut result = self.ask_mesh(req, deadline).await?;
        let mut retries = 0u32;
        loop {
            let remaining = remaining_ms(deadline);
            if result.action != action::FALLBACK_NATIVE
                || !retry::should_retry(result.reason.as_deref(), retries, remaining)
            {
                return Ok(result);
            }
            let Some(delay_ms) = retry::retry_delay_ms(
                result.reason.as_deref(),
                result.retry_after_ms,
                retries,
                remaining,
            ) else {
                self.log.line(&format!(
                    "[ipc] retry skipped id={id}{} retry_after_ms={} -- forwarding fallback",
                    cid_suffix(cid),
                    result.retry_after_ms.map(|v| v.to_string()).unwrap_or_default()
                ));
                return Ok(result);
            };
            self.log.line(&format!(
                "[ipc] retry id={id}{} reason={} attempt={} delay_ms={delay_ms}",
                cid_suffix(cid),
                sanitize_for_console(result.reason.as_deref().unwrap_or("?"), 32),
                retries + 2
            ));
            let wake = Instant::now() + Duration::from_millis(u64::try_from(delay_ms).unwrap_or(0));
            if wake >= deadline {
                tokio::time::sleep_until(deadline).await;
                return Ok(result);
            }
            tokio::time::sleep_until(wake).await;
            req.id = uuid::Uuid::new_v4().simple().to_string();
            retries += 1;
            result = self.ask_mesh(req, deadline).await?;
        }
    }

    async fn ask_mesh(&self, req: &ResolveRequest, deadline: Instant) -> Result<MeshResult, Failure> {
        match tokio::time::timeout_at(deadline, self.mesh.resolve(req, deadline)).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => {
                self.ux.warn(
                    Component::Ipc,
                    &format!(
                        "mesh resolve failed id={}{}: {}",
                        req.id,
                        cid_suffix(req.correlation_id.as_deref()),
                        sanitize_for_console(&e, 160)
                    ),
                );
                Err(Failure::Internal)
            }
            Err(_) => Err(Failure::Deadline),
        }
    }

    fn store_in_cache(&self, req: &ResolveRequest, node_host: &str, frame: &[u8], id: &str, host: &str) {
        let Some(cache) = self.cache.as_deref() else {
            return;
        };
        match serde_json::from_slice::<ResolveResponse>(frame) {
            Ok(parsed) => {
                let default_ttl = parsed.expires_at.as_deref().unwrap_or_default().is_empty();
                cache.store(
                    &Key {
                        node: node_host,
                        url: &req.url,
                        player: req.player.map(Player::as_str),
                        format_arg: req.vrchat_format_arg.as_deref(),
                        max_height: req.max_height,
                    },
                    &parsed,
                );
                self.log.line(&format!(
                    "[resolve-cache] stored id={id} host={host}{}",
                    if default_ttl {
                        " ttl=default"
                    } else {
                        " ttl=server"
                    }
                ));
            }
            Err(e) => self
                .log
                .line(&format!("[resolve-cache] store failed id={id}: {e}")),
        }
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
            self.ux.warn(Component::Wrapper, &sanitize_for_console(msg, 160));
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
        let preview = sanitize_for_console(n.error_preview.as_deref().unwrap_or_default(), 80);
        let mut evicted = 0;
        let mut hint_cleared = false;
        let mut newly_blocked = false;
        if !url.is_empty() {
            evicted = self.cache.as_deref().map(|c| c.evict_by_url(url)).unwrap_or(0);
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
            "[wrapper] wrapper_og_failed rid={} host={host} reason={reason} exit={} elapsed_ms={} evicted={evicted} hint_cleared={} preview={preview}",
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
        let me = self.clone();
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
        loop {
            let mut server = match super::pipe::create(&path, &sddl) {
                Ok(s) => s,
                Err(e) => {
                    self.ux
                        .warn(Component::Ipc, &format!("could not create pipe instance: {e}"));
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            if let Err(e) = server.connect().await {
                self.ux.warn(Component::Ipc, &format!("accept failed: {e}"));
                continue;
            }
            let me = self.clone();
            tokio::spawn(async move {
                me.handle(&mut server).await;
                let _ = server.disconnect();
            });
        }
    }
}

fn remaining_ms(deadline: Instant) -> i64 {
    i64::try_from(deadline.saturating_duration_since(Instant::now()).as_millis()).unwrap_or(i64::MAX)
}
