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
    pub fn new(
        cfg: ShimConfig,
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
    ) -> Self {
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
        self.log(&format!(
            "END exit={exit} outcome={outcome} elapsed_ms={elapsed}"
        ));
        exit
    }

    async fn resolve_or_fall_back(
        &mut self,
        args: &[String],
        url: &str,
        player: Player,
        format_arg: Option<&str>,
    ) -> (i32, &'static str) {
        let mut result = self
            .resolve_over_pipe(url, player, format_arg, None, false)
            .await;
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
        let reason = result
            .fallback_reason
            .unwrap_or(og_reason::PIPE_RESOLVE_FAILED);
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
            self.exec_fallback(args, Some(url), Some(&re_ask), true)
                .await,
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
                        self.log(&format!(
                            "pipe reconnect OK retry={retries} elapsed_ms={elapsed}"
                        ));
                    }
                    break s;
                }
                Err(e) => {
                    let elapsed = ms(started.elapsed());
                    self.log(&match e {
                        ConnectError::TimedOut => {
                            format!(
                                "pipe connect TIMED OUT after {elapsed} ms (watchdog not running?)"
                            )
                        }
                        ConnectError::NotFound => {
                            "pipe connect ENOENT (watchdog not running)".to_owned()
                        }
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
        let rid = if resp.id.is_empty() {
            "?"
        } else {
            id8(&resp.id)
        }
        .to_owned();
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
            self.server_retry_at_ms = resp
                .retry_after_ms
                .map(|r| self.elapsed_ms() + i64::from(r));
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
        elapsed_ms: i64,
    ) {
        let frame = WrapperEventNotify {
            action: action::WRAPPER_OG_FAILED.to_owned(),
            url: Some(url.to_owned()),
            reason: Some(reason.to_owned()),
            elapsed_ms,
            rid: Some(self.log.rid().to_owned()),
            exit_code,
            error_preview: None,
            public_message: None,
        };
        self.send_notify(&frame).await;
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

mod fallback;
