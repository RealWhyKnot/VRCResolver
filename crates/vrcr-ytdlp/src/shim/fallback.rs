use super::*;

impl Shim {
    pub(super) async fn re_ask(&mut self, ctx: &ReAsk<'_>, og_failure: &str) -> Option<String> {
        if og_failure == "content_not_found" {
            self.log("re-ask skipped: content gone upstream");
            return None;
        }
        if budget::re_ask_is_redundant(
            self.server_fallback_reason.as_deref(),
            self.server_retry_at_ms,
        ) {
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
        let window =
            Duration::from_millis(u64::try_from((remaining - 250).max(2000)).unwrap_or(2000));
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

    pub(super) fn terminal_failure(
        &mut self,
        args: &[String],
        url: Option<&str>,
        bundled_reason: &str,
        timed: bool,
    ) {
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

    pub(super) async fn re_ask_or_fail(
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
            self.log(&format!(
                "re-ask resolved without og host={}",
                bare_host(&retry)
            ));
            self.write_url(&retry);
            return 0;
        }
        self.terminal_failure(args, url, reason, timed);
        1
    }

    pub(super) async fn exec_fallback(
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
        let stderr_text = String::from_utf8_lossy(&run.stderr).into_owned();
        let failure = if run.timed_out {
            "timeout"
        } else {
            og::classify_failure(&stderr_text)
        };
        if run.exit != 0
            && let Some(url) = url.filter(|u| !u.is_empty())
        {
            self.notify_og_failed(url, failure, run.exit, og_ms).await;
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
}
