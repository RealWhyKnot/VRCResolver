use super::*;

impl<M: Mesh> Ipc<M> {
    pub(super) async fn resolve<S: AsyncWrite + Unpin>(
        &self,
        pipe: &mut S,
        mut req: ResolveRequest,
        started: Instant,
    ) {
        let id = req.id.clone();
        let mut trace =
            crate::diagnostics::resolve::ResolveTrace::new(&req, &self.stats.playback, &self.ux);
        self.stats
            .playback
            .resolver_origin(&id, &self.mesh.current_node_host());
        let mut cid = req.correlation_id.clone();
        if req.action != action::RESOLVE {
            self.ux.warn(
                Component::Ipc,
                &format!(
                    "rejecting request id={id} action={} -- only \"resolve\" is accepted on this pipe",
                    sanitize_for_console(&req.action, 32)
                ),
            );
            trace.reason = "invalid_request";
            trace.sent = "fallback_native";
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
            trace.reason = "invalid_request";
            trace.sent = "fallback_native";
            framing::write_fallback(pipe, &id, fallback::INTERNAL_ERROR).await;
            return;
        }

        let mut budget_ms = i32::try_from(PER_REQUEST_TIMEOUT.as_millis())
            .expect("request timeout fits milliseconds");
        let mut deadline = started + PER_REQUEST_TIMEOUT;
        if let Some(wrapper_ms) = req.wrapper_deadline_ms {
            budget_ms = effective_budget_ms(wrapper_ms);
            deadline = Instant::now()
                + Duration::from_millis(
                    u64::try_from(budget_ms).expect("request budget is positive"),
                );
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
            trace.reason = "prior_load_failure";
            trace.sent = "fallback_native";
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
                self.ux.success(
                    Component::Ipc,
                    "resolver restored -- mesh resolving re-enabled",
                );
            }
            if paused {
                if let Some(frame) = self.local_frame(&req, &id, deadline).await {
                    trace.reply(&frame, false);
                    let _ = framing::write_frame(pipe, &frame).await;
                    self.ux.write(
                        Component::Ipc,
                        &format!(
                            "local resolve (resolver paused) id={id}{} host={host}",
                            cid_suffix(cid.as_deref())
                        ),
                    );
                    return;
                }
                trace.reason = "resolver_paused";
                trace.sent = "fallback_native";
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
        let mut public_message = None;
        let mut via_cache = false;
        let mut via_local = false;
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
            trace.reply(&hit.frame, true);
            if framing::write_frame(pipe, &hit.frame).await.is_err() {
                trace.finished = false;
                trace.reason = "wrapper_disconnected";
                trace.sent = "partial_or_none";
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
            if req.correlation_id.as_deref().unwrap_or_default().is_empty()
                && caller_opted_into_v2(&req)
            {
                req.correlation_id = Some(req.id.clone());
                cid = req.correlation_id.clone();
            }
            match self
                .resolve_with_retries(&mut req, deadline, cid.as_deref())
                .await
            {
                Ok(mut result) => {
                    let local = if result.action == action::FALLBACK_NATIVE {
                        if let Ok(mut response) =
                            serde_json::from_slice::<ResolveResponse>(&result.frame)
                            && let Some(message) = response.public_message.take()
                            && let Ok(frame) = serde_json::to_vec(&response)
                        {
                            public_message = Some(message);
                            result.frame = frame;
                        }
                        self.local_frame(&req, &id, deadline).await
                    } else {
                        None
                    };
                    via_local = local.is_some();
                    let frame = local.as_deref().unwrap_or(&result.frame);
                    trace.retried = req.id != trace.request.id;
                    trace.reply(frame, false);
                    if framing::write_frame(pipe, frame).await.is_err() {
                        trace.finished = false;
                        trace.reason = "wrapper_disconnected";
                        trace.sent = "partial_or_none";
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
            trace.reason = reason;
            trace.sent = "fallback_native";
            trace.retried = req.id != trace.request.id;
            outcome = format!("{}/{reason}", action::FALLBACK_NATIVE);
            let local = if matches!(failure, Some(Failure::Internal)) {
                self.local_frame(&req, &id, deadline).await
            } else {
                None
            };
            match local {
                Some(frame) => {
                    via_local = true;
                    trace.reply(&frame, false);
                    let _ = framing::write_frame(pipe, &frame).await;
                }
                None => framing::write_fallback(pipe, &id, reason).await,
            }
            (self.report_fallback)(&req, reason);
        } else if let Some(reason) = outcome
            .strip_prefix(action::FALLBACK_NATIVE)
            .and_then(|r| r.strip_prefix('/'))
            .filter(|r| !r.is_empty())
        {
            (self.report_fallback)(&req, reason);
        }

        let (status, reason_for_line) = if via_local {
            (
                ResolveStatus::Local,
                fail_reason
                    .map(str::to_owned)
                    .or_else(|| server_reason.clone()),
            )
        } else if outcome == action::RESOLVED {
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
        self.stats.perf.record_resolve(status, via_cache, elapsed);
        self.ux.resolve_outcome(&Outcome {
            host: &host,
            player: &player_label,
            status,
            via_cache,
            via_lh_yt,
            elapsed,
            reason: reason_for_line.as_deref(),
            public_message: public_message.as_deref(),
        });

        if let Some(gate) = self.health.as_deref()
            && !via_cache
        {
            let healthy =
                is_healthy_outcome(fail_reason, &outcome, server_reason.as_deref(), budget_ms);
            match gate.record_resolve_outcome(crate::health::ResolveOutcome { healthy, resolved: outcome == action::RESOLVED }) {
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
    }

    async fn local_frame(
        &self,
        req: &ResolveRequest,
        id: &str,
        deadline: Instant,
    ) -> Option<Vec<u8>> {
        let step = self.local.as_deref()?;
        if req.player != Some(Player::AvPro) || !step.covers(&req.url) {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining < local::MIN_BUDGET {
            self.log
                .line(&format!("[local] skipped id={id} reason=no_time"));
            return None;
        }
        let budget = (remaining - local::BUDGET_RESERVE).min(local::MAX_BUDGET);
        let started = Instant::now();
        let ask = Ask {
            url: &req.url,
            max_height: req.max_height,
        };
        let attempt = tokio::time::timeout(budget, step.play(ask)).await;
        let elapsed_ms = started.elapsed().as_millis();
        let play = match attempt {
            Ok(Ok(play)) => play,
            Ok(Err(reason)) => {
                self.log.line(&format!(
                    "[local] skipped id={id} reason={reason} elapsed_ms={elapsed_ms}"
                ));
                return None;
            }
            Err(_) => {
                self.log.line(&format!(
                    "[local] skipped id={id} reason=timeout elapsed_ms={elapsed_ms}"
                ));
                return None;
            }
        };
        self.log.line(&format!(
            "[local] resolved id={id} height={} elapsed_ms={elapsed_ms}",
            play.height
        ));
        let frame = ResolveResponse {
            action: action::RESOLVED.to_owned(),
            id: id.to_owned(),
            url: Some(play.url),
            engine: Some(local::ENGINE.to_owned()),
            protocol: Some("hls".to_owned()),
            video_codec: Some("h264".to_owned()),
            audio_codec: play.audio_codec.map(|_| "aac".to_owned()),
            resolved_height: i32::try_from(play.height).ok(),
            ..ResolveResponse::default()
        };
        serde_json::to_vec(&frame).ok()
    }

    pub(super) async fn resolve_with_retries(
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
                    result
                        .retry_after_ms
                        .map(|v| v.to_string())
                        .unwrap_or_default()
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

    pub(super) async fn ask_mesh(
        &self,
        req: &ResolveRequest,
        deadline: Instant,
    ) -> Result<MeshResult, Failure> {
        match tokio::time::timeout_at(deadline, self.mesh.resolve(req, deadline)).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) => Err(Failure::Internal),
            Err(_) => Err(Failure::Deadline),
        }
    }

    pub(super) fn store_in_cache(
        &self,
        req: &ResolveRequest,
        node_host: &str,
        frame: &[u8],
        id: &str,
        host: &str,
    ) {
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
}
