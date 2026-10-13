use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite};
use vrcr_shared::budget;
use vrcr_shared::fallback_binary;
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::logutil::bare_host;
use vrcr_shared::paths::AppPaths;
use vrcr_shared::profile;
use vrcr_shared::relay;
use vrcr_shared::trust_gateway;
use vrcr_shared::url_guard;
use vrcr_shared::wire::{self, Player, ResolveResponse, WrapperEventNotify, action, og_reason};
use vrcr_shared::wrapper_identity::{self, WrapperKind};

use crate::log::{self, ShimLog};
use crate::og;
use crate::pipe::{self, ConnectError};

pub struct ShimConfig {
    pub pipe_name: String,
    pub state_root: PathBuf,
    pub exe_dir: PathBuf,
    pub vrc_tools_dir: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    pub version: String,
}

impl ShimConfig {
    pub fn from_env(version: &str) -> Self {
        let paths = AppPaths::from_env();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_default();
        Self {
            pipe_name: wire::PIPE_NAME.to_owned(),
            state_root: paths.state_root(),
            exe_dir,
            vrc_tools_dir: paths.vrc_tools_dir(None),
            log_path: Some(paths.logs_dir().join(log::LOG_FILE)),
            version: version.to_owned(),
        }
    }
}

pub struct PipeOutcome {
    pub url: Option<String>,
    pub fallback_reason: Option<&'static str>,
    pub public_message: Option<String>,
}

impl PipeOutcome {
    fn failed(reason: &'static str) -> Self {
        Self {
            url: None,
            fallback_reason: Some(reason),
            public_message: None,
        }
    }
}

struct ReAsk<'a> {
    url: &'a str,
    player: Player,
    format_arg: Option<&'a str>,
}

pub struct Shim {
    cfg: ShimConfig,
    policy: FirstPartyPolicy,
    log: ShimLog,
    clock: Instant,
    pipe_unavailable: bool,
    server_retry_at_ms: Option<i64>,
    server_fallback_reason: Option<String>,
    last_request_id: Option<String>,
    stdout: Box<dyn Write + Send>,
    stderr: Box<dyn Write + Send>,
}

fn id8(id: &str) -> &str {
    id.char_indices().nth(8).map_or(id, |(cut, _)| &id[..cut])
}

fn ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

impl Shim {
    pub fn new(cfg: ShimConfig, stdout: Box<dyn Write + Send>, stderr: Box<dyn Write + Send>) -> Self {
        let rid = profile::new_request_id()[..8].to_owned();
        let log = ShimLog::new(cfg.log_path.clone(), rid);
        Self {
            cfg,
            policy: FirstPartyPolicy::default(),
            log,
            clock: Instant::now(),
            pipe_unavailable: false,
            server_retry_at_ms: None,
            server_fallback_reason: None,
            last_request_id: None,
            stdout,
            stderr,
        }
    }

    fn elapsed_ms(&self) -> i64 {
        ms(self.clock.elapsed())
    }

    fn log(&mut self, message: &str) {
        self.log.line(message);
    }

    pub async fn run(&mut self, args: &[String]) -> i32 {
        let url = log::extract_url(args).to_owned();
        let format_arg = profile::dash_f_value(args).map(str::to_owned);
        let player = profile::infer_player(format_arg.as_deref());
        let banner = log::start_banner(
            std::process::id(),
            &self.cfg.version,
            args,
            &url,
            format_arg.as_deref(),
            player.as_str(),
        );
        self.log(&banner);
        let (exit, outcome) = if url.is_empty() {
            self.log("no URL in argv -- exec og fallback (diagnostic invocation)");
            let elapsed = self.elapsed_ms();
            self.notify_og_fallback(None, og_reason::NO_URL_DIAGNOSTIC, elapsed, None)
                .await;
            (
                self.exec_fallback(args, None, None, false).await,
                "no-url-fallback",
            )
        } else if self.policy.is_playback_proxy_url(&url) {
            let emitted = self.wrap_for_gateway(&url, true);
            self.write_url(&emitted);
            let wrapped = emitted != url;
            self.log(&format!(
                "direct first-party playback URL host={} emitted-host={} bytes={} trust_gateway={}",
                bare_host(&url),
                bare_host(&emitted),
                emitted.len(),
                if wrapped { "wrapped" } else { "passthrough" }
            ));
            (
                0,
                if wrapped {
                    "direct-first-party-playback-wrapped"
                } else {
                    "direct-first-party-playback"
                },
            )
        } else {
            self.resolve_or_fall_back(args, &url, player, format_arg.as_deref())
                .await
        };
        let elapsed = self.elapsed_ms();
        self.log(&format!("END exit={exit} outcome={outcome} elapsed_ms={elapsed}"));
        exit
    }

    async fn resolve_or_fall_back(
        &mut self,
        args: &[String],
        url: &str,
        player: Player,
        format_arg: Option<&str>,
    ) -> (i32, &'static str) {
        let mut result = self.resolve_over_pipe(url, player, format_arg, None, false).await;
        if let Some(resolved) = result.url.as_deref()
            && !url_guard::is_safe_to_emit(Some(resolved))
        {
            self.log(&format!(
                "resolved URL fails emit guard (host={}) -- falling back",
                bare_host(resolved)
            ));
            result = PipeOutcome::failed(og_reason::RESOLVED_URL_REJECTED);
        }
        if let Some(resolved) = result.url.filter(|u| !u.is_empty()) {
            let emitted = self.wrap_for_gateway(&resolved, false);
            self.write_url(&emitted);
            let wrapped = emitted != resolved;
            self.log(&format!(
                "emitted resolved URL to stdout host={} bytes={} trust_gateway={}",
                bare_host(&emitted),
                emitted.len(),
                if wrapped { "wrapped" } else { "passthrough" }
            ));
            return (
                0,
                if wrapped {
                    "pipe-resolved-wrapped"
                } else {
                    "pipe-resolved"
                },
            );
        }
        let reason = result.fallback_reason.unwrap_or(og_reason::PIPE_RESOLVE_FAILED);
        if self.server_fallback_reason.is_none() {
            self.server_fallback_reason = Some(reason.to_owned());
        }
        let elapsed = self.elapsed_ms();
        self.notify_og_fallback(Some(url), reason, elapsed, result.public_message)
            .await;
        let re_ask = ReAsk {
            url,
            player,
            format_arg,
        };
        (
            self.exec_fallback(args, Some(url), Some(&re_ask), true).await,
            "pipe-failed-og-fallback",
        )
    }

    pub async fn resolve_over_pipe(
        &mut self,
        url: &str,
        player: Player,
        format_arg: Option<&str>,
        deadline: Option<Duration>,
        skip_native_hint: bool,
    ) -> PipeOutcome {
        let started = Instant::now();
        let total_ms = ms(deadline.unwrap_or(budget::TOTAL));
        let mut req = profile::wrapper_request(
            profile::new_request_id(),
            url,
            player,
            format_arg,
            skip_native_hint,
        );
        self.last_request_id = Some(req.id.clone());
        let mut retries = 0;
        let mut stream = loop {
            match pipe::connect(&self.cfg.pipe_name, pipe::CONNECT_TIMEOUT).await {
                Ok(s) => {
                    let elapsed = ms(started.elapsed());
                    if retries == 0 {
                        self.log(&format!("pipe connect OK elapsed_ms={elapsed}"));
                    } else {
                        self.log(&format!("pipe reconnect OK retry={retries} elapsed_ms={elapsed}"));
                    }
                    break s;
                }
                Err(e) => {
                    let elapsed = ms(started.elapsed());
                    self.log(&match e {
                        ConnectError::TimedOut => {
                            format!("pipe connect TIMED OUT after {elapsed} ms (watchdog not running?)")
                        }
                        ConnectError::NotFound => "pipe connect ENOENT (watchdog not running)".to_owned(),
                        ConnectError::Other(e) => format!("pipe connect failed: {e:?}"),
                    });
                    if retries == 0 && total_ms - elapsed > 2000 {
                        retries += 1;
                        self.log("pipe connect retry in 750 ms");
                        tokio::time::sleep(Duration::from_millis(750)).await;
                        continue;
                    }
                    self.pipe_unavailable = true;
                    return PipeOutcome::failed(og_reason::PIPE_CONNECT_FAILED);
                }
            }
        };
        let remaining = total_ms - ms(started.elapsed());
        if remaining <= 0 {
            let elapsed = ms(started.elapsed());
            self.log(&format!(
                "resolve budget exhausted before attempt elapsed_ms={elapsed}"
            ));
            return PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED);
        }
        let attempt = Duration::from_millis(u64::try_from(remaining).unwrap_or(0));
        let wrapper_deadline = (total_ms - ms(started.elapsed()) - 1000).max(0);
        req.wrapper_deadline_ms = Some(i32::try_from(wrapper_deadline).unwrap_or(i32::MAX));
        let exchange = self.exchange(&mut stream, &req, player, total_ms, started);
        match tokio::time::timeout(attempt, exchange).await {
            Ok(outcome) => outcome,
            Err(_) => {
                self.log(&format!(
                    "pipe read TIMED OUT after {} ms (no terminal frame within {total_ms} ms)",
                    ms(started.elapsed())
                ));
                PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED)
            }
        }
    }

    async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
        req: &wire::ResolveRequest,
        player: Player,
        total_ms: i64,
        started: Instant,
    ) -> PipeOutcome {
        let payload = match wire::to_line(req) {
            Ok(p) => p,
            Err(e) => {
                self.log(&format!("request serialize failed: {e}"));
                return PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED);
            }
        };
        let send_started = Instant::now();
        if let Err(e) = pipe::send_frame(stream, &payload).await {
            self.log(&format!("pipe write failed: {:?}: {e}", e.kind()));
            return PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED);
        }
        self.log(&format!(
            "request sent id={} bytes={} player={} wrapper_deadline_ms={} elapsed_ms={}",
            id8(&req.id),
            payload.len() - 1,
            player.as_str(),
            req.wrapper_deadline_ms.unwrap_or(0),
            ms(send_started.elapsed())
        ));
        let request_sent = Instant::now();
        let read_started = Instant::now();
        let line = match pipe::read_line(stream).await {
            Ok(Some(l)) => l,
            Ok(None) => {
                self.log(&format!(
                    "pipe returned empty response after {} ms",
                    ms(read_started.elapsed())
                ));
                return PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED);
            }
            Err(e) => {
                self.log(&format!("pipe read failed: {:?}: {e}", e.kind()));
                return PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED);
            }
        };
        self.log(&format!(
            "response received bytes={} elapsed_ms={}",
            line.len(),
            ms(read_started.elapsed())
        ));
        let resp: ResolveResponse = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                self.log(&format!("response parse failed: {e}"));
                return PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED);
            }
        };
        let rid = if resp.id.is_empty() { "?" } else { id8(&resp.id) }.to_owned();
        if resp.action == action::RESOLVED
            && let Some(url) = resp.url.filter(|u| !u.is_empty())
        {
            if player == Player::AvPro && !url_guard::is_avpro_compatible_url(&url) {
                self.log(&format!(
                    "response action=resolved id={rid} but URL fails AVPro shape check (host={}) -- falling back",
                    bare_host(&url)
                ));
                return PipeOutcome::failed(og_reason::AVPRO_INCOMPATIBLE);
            }
            self.log(&format!(
                "response action=resolved id={rid} url-host={}",
                bare_host(&url)
            ));
            return PipeOutcome {
                url: Some(url),
                fallback_reason: None,
                public_message: None,
            };
        }
        if resp.action == action::FALLBACK_NATIVE {
            self.server_retry_at_ms = resp.retry_after_ms.map(|r| self.elapsed_ms() + i64::from(r));
            if !req.skip_native_hint.unwrap_or(false) {
                self.server_fallback_reason.clone_from(&resp.reason);
            }
            self.log(&format!(
                "response action=fallback_native id={rid} reason={} retry_after_ms={} elapsed_ms_since_request_sent={} remaining_budget_ms={}",
                resp.reason.as_deref().unwrap_or("?"),
                resp.retry_after_ms.map_or_else(|| "-".to_owned(), |r| r.to_string()),
                ms(request_sent.elapsed()),
                total_ms - ms(started.elapsed())
            ));
            return PipeOutcome {
                url: None,
                fallback_reason: Some(og_reason::SERVER_FALLBACK_NATIVE),
                public_message: resp.public_message,
            };
        }
        self.log(&format!("response action UNKNOWN: {}", resp.action));
        PipeOutcome::failed(og_reason::PIPE_RESOLVE_FAILED)
    }

    async fn send_notify(&mut self, frame: &WrapperEventNotify) {
        if self.pipe_unavailable {
            return;
        }
        let Ok(line) = wire::to_line(frame) else {
            return;
        };
        let name = self.cfg.pipe_name.clone();
        let _ = tokio::time::timeout(pipe::NOTIFY_TIMEOUT, async move {
            if let Ok(mut s) = pipe::connect(&name, pipe::NOTIFY_TIMEOUT).await {
                let _ = pipe::send_frame(&mut s, &line).await;
            }
        })
        .await;
    }

    pub async fn notify_og_fallback(
        &mut self,
        url: Option<&str>,
        reason: &str,
        elapsed_ms: i64,
        public_message: Option<String>,
    ) {
        let frame = WrapperEventNotify {
            action: action::OG_FALLBACK_NOTIFY.to_owned(),
            url: url.map(str::to_owned),
            reason: Some(reason.to_owned()),
            elapsed_ms,
            rid: Some(self.log.rid().to_owned()),
            exit_code: 0,
            error_preview: None,
            public_message,
        };
        self.send_notify(&frame).await;
    }

    pub async fn notify_og_failed(
        &mut self,
        url: &str,
        reason: &str,
        exit_code: i32,
        preview: String,
        elapsed_ms: i64,
    ) {
        let frame = WrapperEventNotify {
            action: action::WRAPPER_OG_FAILED.to_owned(),
            url: Some(url.to_owned()),
            reason: Some(reason.to_owned()),
            elapsed_ms,
            rid: Some(self.log.rid().to_owned()),
            exit_code,
            error_preview: Some(preview),
            public_message: None,
        };
        self.send_notify(&frame).await;
    }

    async fn re_ask(&mut self, ctx: &ReAsk<'_>, og_failure: &str) -> Option<String> {
        if og_failure == "content_not_found" {
            self.log("re-ask skipped: content gone upstream");
            return None;
        }
        if budget::re_ask_is_redundant(self.server_fallback_reason.as_deref(), self.server_retry_at_ms) {
            let reason = self.server_fallback_reason.clone().unwrap_or_default();
            self.log(&format!(
                "re-ask skipped: server already re-raced reason={reason}"
            ));
            return None;
        }
        let elapsed = self.elapsed_ms();
        let Some(delay) = budget::re_ask_delay_ms(elapsed, self.server_retry_at_ms) else {
            let at = self
                .server_retry_at_ms
                .map_or_else(|| "-".to_owned(), |v| v.to_string());
            self.log(&format!(
                "re-ask skipped: elapsed_ms={elapsed} retry_at_elapsed_ms={at}"
            ));
            return None;
        };
        if delay > 0 {
            self.log(&format!("re-ask waiting delay_ms={delay} per server hint"));
            tokio::time::sleep(Duration::from_millis(u64::try_from(delay).unwrap_or(0))).await;
        }
        let remaining = budget::TOTAL_MS - self.elapsed_ms();
        self.log(&format!("re-ask start remaining_budget_ms={remaining}"));
        let window = Duration::from_millis(u64::try_from((remaining - 250).max(2000)).unwrap_or(2000));
        let retry = self
            .resolve_over_pipe(ctx.url, ctx.player, ctx.format_arg, Some(window), true)
            .await;
        let Some(url) = retry.url.filter(|u| !u.is_empty()) else {
            self.log(&format!(
                "re-ask did not resolve reason={}",
                retry.fallback_reason.unwrap_or("?")
            ));
            return None;
        };
        if !url_guard::is_safe_to_emit(Some(&url)) {
            self.log(&format!(
                "re-ask resolved URL fails emit guard host={}",
                bare_host(&url)
            ));
            return None;
        }
        Some(self.wrap_for_gateway(&url, false))
    }

    fn terminal_failure(&mut self, args: &[String], url: Option<&str>, bundled_reason: &str, timed: bool) {
        let player = profile::infer_player(profile::dash_f_value(args));
        let line = relay::terminal_failure_line(
            self.last_request_id.as_deref().unwrap_or(self.log.rid()),
            &url.filter(|u| !u.is_empty())
                .map_or_else(|| "?".to_owned(), bare_host),
            player.as_str(),
            self.server_fallback_reason.as_deref().unwrap_or("?"),
            bundled_reason,
            if timed { self.elapsed_ms() } else { 0 },
        );
        self.log(&line);
    }

    async fn re_ask_or_fail(
        &mut self,
        args: &[String],
        url: Option<&str>,
        re_ask: Option<&ReAsk<'_>>,
        timed: bool,
        reason: &str,
    ) -> i32 {
        if let Some(ctx) = re_ask
            && let Some(retry) = self.re_ask(ctx, reason).await
        {
            self.log(&format!("re-ask resolved without og host={}", bare_host(&retry)));
            self.write_url(&retry);
            return 0;
        }
        self.terminal_failure(args, url, reason, timed);
        1
    }

    async fn exec_fallback(
        &mut self,
        args: &[String],
        url: Option<&str>,
        re_ask: Option<&ReAsk<'_>>,
        timed: bool,
    ) -> i32 {
        let known_hashes = self.cfg.state_root.join("wrapper_hashes.txt");
        let og_path = fallback_binary::select(
            &self.cfg.exe_dir,
            self.cfg.vrc_tools_dir.as_deref(),
            |p| p.is_file(),
            |p| wrapper_identity::classify(p, Some(&known_hashes)) == WrapperKind::Ours,
        );
        let Some(og_path) = og_path else {
            self.log("FALLBACK no-og: no usable vanilla yt-dlp found");
            return self
                .re_ask_or_fail(args, url, re_ask, timed, "no_og_binary")
                .await;
        };
        self.log(&format!(
            "FALLBACK og: spawning {} with {} args",
            og_path.display(),
            args.len()
        ));
        let elapsed = if timed {
            self.clock.elapsed()
        } else {
            Duration::ZERO
        };
        let og_budget = budget::og_window(elapsed, re_ask.is_some(), self.server_retry_at_ms);
        let working_dir = og_path
            .parent()
            .map_or_else(|| self.cfg.exe_dir.clone(), PathBuf::from);
        let started = Instant::now();
        let run = match og::run(&og_path, args, &working_dir, og_budget).await {
            Ok(r) => r,
            Err(e) => {
                self.log(&format!(
                    "FALLBACK og: exec failed: {:?}: {e} elapsed_ms={}",
                    e.kind(),
                    ms(started.elapsed())
                ));
                return self
                    .re_ask_or_fail(args, url, re_ask, timed, "og_exec_failed")
                    .await;
            }
        };
        let og_ms = ms(started.elapsed());
        self.log(&format!(
            "FALLBACK og: exit={} timed_out={} budget_ms={} elapsed_ms={og_ms} stdout_bytes={} stderr_bytes={}",
            run.exit,
            run.timed_out,
            og_budget.as_millis(),
            run.stdout.len(),
            run.stderr.len()
        ));
        let stdout_text = String::from_utf8_lossy(&run.stdout).into_owned();
        let stderr_text = String::from_utf8_lossy(&run.stderr).into_owned();
        if !stdout_text.is_empty() {
            self.log(&format!(
                "FALLBACK og stdout-preview: {}",
                log::preview(&stdout_text, 240)
            ));
        }
        if !stderr_text.is_empty() {
            self.log(&format!(
                "FALLBACK og stderr-preview: {}",
                log::preview(&stderr_text, 240)
            ));
        }
        let failure = if run.timed_out {
            "timeout"
        } else {
            og::classify_failure(&stderr_text)
        };
        if run.exit != 0
            && let Some(url) = url.filter(|u| !u.is_empty())
        {
            let preview = if stderr_text.is_empty() {
                String::new()
            } else {
                log::preview(stderr_text.trim(), 200)
            };
            self.notify_og_failed(url, failure, run.exit, preview, og_ms)
                .await;
            if let Some(ctx) = re_ask
                && let Some(retry) = self.re_ask(ctx, failure).await
            {
                self.log(&format!(
                    "re-ask resolved after og failure host={} suppressed_og_stderr_bytes={}",
                    bare_host(&retry),
                    run.stderr.len()
                ));
                self.write_url(&retry);
                return 0;
            }
        }
        if !run.stdout.is_empty() {
            let _ = self.stdout.write_all(&run.stdout);
            let _ = self.stdout.flush();
        }
        if !run.stderr.is_empty() {
            let _ = self.stderr.write_all(&run.stderr);
            let _ = self.stderr.flush();
        }
        if run.exit != 0 {
            self.terminal_failure(args, url, failure, timed);
        }
        run.exit
    }

    fn wrap_for_gateway(&self, url: &str, probe: bool) -> String {
        trust_gateway::wrap_for_relay(&self.policy, &self.cfg.state_root, url, |port| {
            !probe || relay::is_listening(port, relay::DEFAULT_PROBE_TIMEOUT)
        })
    }

    fn write_url(&mut self, url: &str) {
        let line = format!("{}\n", url.trim());
        let written = self
            .stdout
            .write_all(line.as_bytes())
            .and_then(|()| self.stdout.flush());
        if let Err(e) = written {
            self.log(&format!("stdout write failed: {:?}: {e}", e.kind()));
        }
    }
}
