use super::*;

impl Inner {
    pub(super) fn parse_failure_count(&self, key: &str) -> Option<u32> {
        let now = Self::now();
        let mut map = lock(&self.parse_failures);
        let count = match map.get_mut(key) {
            Some((last, count)) => {
                *count += 1;
                if now.duration_since(*last) >= PARSE_FAILURE_WINDOW {
                    *last = now;
                    Some(*count)
                } else {
                    None
                }
            }
            None => {
                map.insert(key.to_owned(), (now, 1));
                Some(1)
            }
        };
        drop(map);
        count
    }

    pub(super) async fn send_pong(&self, link: Option<&Arc<Link>>) {
        if let Some(link) = link.filter(|l| l.is_open()) {
            let _ = self.send_text(link, frames::PONG_FRAME.to_owned()).await;
        }
    }

    pub(super) fn abort_link(link: Option<&Arc<Link>>) {
        if let Some(link) = link {
            link.abort();
        }
    }

    pub(super) fn settle_welcome(link: Option<&Arc<Link>>) {
        if let Some(link) = link {
            link.settle_welcome();
        }
    }

    pub(super) fn apply_first_party(
        &self,
        features: Option<&[String]>,
        hosts: Option<&[String]>,
        paths: Option<&[String]>,
    ) {
        if has_feature(features, feature::WELCOME_HOSTS) {
            let policy = FirstPartyPolicy::with_server_provided(hosts, paths);
            *self.first_party.write().unwrap_or_else(|e| e.into_inner()) = policy;
        }
    }

    pub(super) fn finish_resolve(
        &self,
        id: &str,
        resolved_url: Option<&str>,
        resolved: bool,
        result: MeshResult,
    ) {
        if id.is_empty() {
            return;
        }
        let cid = lock(&self.inflight_cids).remove(id);
        if resolved && let (Some(cid), Some(url)) = (cid.as_deref(), resolved_url) {
            self.remember_resolved_url_cid(url, cid);
        }
        let mut sink = lock(&self.pending);
        if let Some(tx) = sink.remove(id) {
            drop(sink);
            if resolved {
                self.reconnect_attempt.store(0, Ordering::SeqCst);
            }
            let _ = tx.send(result);
        }
    }

    pub(super) async fn dispatch_text(self: &Arc<Self>, link: Option<&Arc<Link>>, payload: &[u8]) {
        let root: Value = match serde_json::from_slice(payload) {
            Ok(v) => v,
            Err(e) => {
                if let Some(count) = self.parse_failure_count("json") {
                    self.ux.log_warn(
                        Component::Mesh,
                        &format!(
                            "frame parse failed (json x{count} in last min): {:?} line={} column={} bytes={}",
                            e.classify(),
                            e.line(),
                            e.column(),
                            payload.len()
                        ),
                    );
                }
                return;
            }
        };
        let name = frames::str_field(&root, "action")
            .unwrap_or_default()
            .to_owned();
        match name.as_str() {
            action::RESOLVED | action::FALLBACK_NATIVE => {
                let resolved = name == action::RESOLVED;
                let id = frames::str_field(&root, "id").unwrap_or_default();
                let reason = frames::str_field(&root, "reason").map(str::to_owned);
                let retry_after_ms = if resolved {
                    None
                } else {
                    frames::i32_field(&root, "retry_after_ms")
                };
                let url = if resolved {
                    frames::str_field(&root, "url")
                } else {
                    None
                };
                if resolved && let Some(bytes) = root.get("bytes_estimate").and_then(Value::as_i64)
                {
                    self.stats.record_bytes_estimate(bytes);
                }
                if !resolved {
                    self.log
                        .line(&frames::fallback_native_line(id, reason.as_deref()));
                }
                let result = MeshResult {
                    frame: payload.to_vec(),
                    action: name.clone(),
                    reason,
                    retry_after_ms,
                };
                self.finish_resolve(id, url.filter(|u| !u.is_empty()), resolved, result);
            }
            action::WELCOME => self.on_welcome(link, payload),
            action::WELCOME_CACHED => self.on_welcome_cached(link, payload),
            action::RESOLVE_LOG => self.log.line(&frames::resolve_log_line(&root)),
            action::PLAYBACK_QUALITY_UPDATE => self.log.line(&frames::playback_quality_line(&root)),
            action::RATE_LIMITED => self.on_rate_limited(&root),
            action::PROTOCOL_ERROR => self.on_protocol_error(&root),
            action::PONG => *lock(&self.last_pong) = Some(Self::now()),
            action::PING => self.send_pong(link).await,
            other => self.ux.log_warn(
                Component::Mesh,
                &format!(
                    "unknown action -- discarding: {}",
                    sanitize_for_console(other, 64)
                ),
            ),
        }
    }

    pub(super) fn on_welcome(&self, link: Option<&Arc<Link>>, payload: &[u8]) {
        let welcome: WelcomeFrame = match serde_json::from_slice(payload) {
            Ok(w) => w,
            Err(e) => {
                self.ux.log_warn(
                    Component::Mesh,
                    &format!(
                        "welcome parse failed -- assuming v1 server: {:?} line={} column={} bytes={}",
                        e.classify(),
                        e.line(),
                        e.column(),
                        payload.len()
                    ),
                );
                {
                    let mut s = lock(&self.server);
                    if s.protocol_version == 0 {
                        s.protocol_version = 1;
                    }
                }
                Self::settle_welcome(link);
                return;
            }
        };
        let negotiated = welcome
            .protocol_version
            .clamp(1, wire::CLIENT_PROTOCOL_VERSION);
        let negotiated_format = welcome
            .negotiated_format
            .clone()
            .unwrap_or_else(|| format::JSON.to_owned());
        let is_msgpack = negotiated_format == format::MSGPACK;
        let (is_v3, host) = {
            let mut s = lock(&self.server);
            s.protocol_version = negotiated;
            s.node = welcome.node.clone();
            s.features = welcome.features.clone();
            s.warp_active = welcome.warp_active;
            s.server_version = welcome.server_version.clone();
            s.yt_dlp_version = welcome.yt_dlp_version.clone();
            s.negotiated_format = negotiated_format.clone();
            s.is_msgpack = is_msgpack;
            (s.is_v3, s.current_host.clone())
        };
        self.log.line(&format!(
            "[mesh][v3.1] negotiated_format={negotiated_format} isMsgpack={is_msgpack}"
        ));
        if welcome.features.is_none() {
            self.ux
                .log_warn(Component::Mesh, "welcome missing required field: features");
        }
        self.apply_first_party(
            welcome.features.as_deref(),
            welcome.first_party_hosts.as_deref(),
            welcome.playback_proxy_paths.as_deref(),
        );
        let features = match welcome.features.as_deref() {
            Some(f) if !f.is_empty() => format!("[{}]", f.join(",")),
            _ => "[]".to_owned(),
        };
        self.ux.log_write(
            Component::Mesh,
            &format!(
                "welcome node={} v={} (negotiated={negotiated}) server={} yt-dlp={} warp_active={} features={}",
                welcome.node.as_deref().unwrap_or("?"),
                welcome.protocol_version,
                welcome.server_version.as_deref().unwrap_or("?"),
                welcome.yt_dlp_version.as_deref().unwrap_or("?"),
                welcome.warp_active.map_or_else(|| "?".to_owned(), |w| w.to_string()),
                sanitize_for_console(&features, 240)
            ),
        );
        if is_v3
            && !host.is_empty()
            && let Some(hash) = welcome.welcome_hash.as_deref().filter(|h| !h.is_empty())
        {
            self.welcome_cache.store(&host, &welcome, hash);
        }
        Self::settle_welcome(link);
    }

    pub(super) fn on_welcome_cached(&self, link: Option<&Arc<Link>>, payload: &[u8]) {
        let (is_v3, host) = {
            let s = lock(&self.server);
            (s.is_v3, s.current_host.clone())
        };
        if !is_v3 {
            self.ux.log_warn(
                Component::Mesh,
                "welcome_cached received on non-v3 connection -- protocol error, reconnecting",
            );
            Self::abort_link(link);
            return;
        }
        let cached: WelcomeCachedFrame = match serde_json::from_slice(payload) {
            Ok(c) => c,
            Err(e) => {
                self.ux.log_warn(
                    Component::Mesh,
                    &format!(
                        "welcome_cached parse failed: {:?} line={} column={} bytes={}",
                        e.classify(),
                        e.line(),
                        e.column(),
                        payload.len()
                    ),
                );
                {
                    let mut s = lock(&self.server);
                    if s.protocol_version == 0 {
                        s.protocol_version = 1;
                    }
                }
                Self::settle_welcome(link);
                return;
            }
        };
        let Some(entry) = self.welcome_cache.get(&host) else {
            self.welcome_cache.invalidate(&host);
            self.ux.log_warn(
                Component::Mesh,
                "welcome_cached but local entry missing -- invalidating + reconnecting",
            );
            Self::abort_link(link);
            return;
        };
        let negotiated = cached
            .protocol_version
            .clamp(1, wire::CLIENT_PROTOCOL_VERSION);
        let negotiated_format = cached
            .negotiated_format
            .clone()
            .unwrap_or_else(|| format::JSON.to_owned());
        let node = cached.node.clone().or_else(|| entry.node.clone());
        {
            let mut s = lock(&self.server);
            s.protocol_version = negotiated;
            s.node = node.clone();
            s.features = entry.features.clone();
            s.warp_active = cached.warp_active.or(entry.warp_active);
            s.server_version = entry.server_version.clone();
            s.yt_dlp_version = entry.yt_dlp_version.clone();
            s.is_msgpack = negotiated_format == format::MSGPACK;
            s.negotiated_format = negotiated_format.clone();
        }
        self.apply_first_party(
            entry.features.as_deref(),
            entry.first_party_hosts.as_deref(),
            entry.playback_proxy_paths.as_deref(),
        );
        self.log.line(&format!(
            "[mesh][v3] welcome_cached hit node={} v={negotiated} negotiated_format={negotiated_format} features={}",
            node.as_deref().unwrap_or("?"),
            entry
                .features
                .as_deref()
                .map_or_else(|| "<none>".to_owned(), |f| f.join(","))
        ));
        Self::settle_welcome(link);
    }

    pub(super) fn on_rate_limited(&self, root: &Value) {
        let mesh_action = frames::str_field(root, wire::field::MESH_ACTION).unwrap_or_default();
        let limited_id = frames::str_field(root, "id").filter(|i| !i.is_empty());
        let retry_after = frames::i32_field(root, wire::field::RETRY_AFTER_SECONDS).unwrap_or(0);
        let cooldown = retry_after.clamp(1, MAX_RATE_LIMIT_COOLDOWN_SECONDS);
        self.ux.log_warn(
            Component::Mesh,
            &format!(
                "server rate-limited {} -- pausing for {cooldown}s",
                sanitize_for_console(mesh_action, 32)
            ),
        );
        self.ux.notice(
            vrcr_shared::console::Color::Yellow,
            &format!("The server asked VRCResolver to slow down. Waiting {cooldown}s."),
        );
        if mesh_action != action::RESOLVE {
            return;
        }
        let secs = u64::try_from(cooldown).unwrap_or(1);
        *lock(&self.rate_limited_until) = Some(Self::now() + Duration::from_secs(secs));
        match limited_id {
            Some(id) => {
                lock(&self.inflight_cids).remove(id);
                self.complete(id, frames::fallback_result(id, fallback::RATE_LIMITED));
            }
            None => self.fail_all_pending(fallback::RATE_LIMITED),
        }
    }

    pub(super) fn on_protocol_error(&self, root: &Value) {
        let reason = frames::str_field(root, "reason").unwrap_or_default();
        let field = frames::str_field(root, "field").unwrap_or_default();
        let id = frames::str_field(root, "id");
        let mut line = format!(
            "server protocol_error reason={}",
            sanitize_for_console(reason, 48)
        );
        if !field.is_empty() {
            let _ = write!(line, " field={}", sanitize_for_console(field, 48));
        }
        if let Some(id) = id {
            let _ = write!(line, " id={}", sanitize_for_console(id, 32));
        }
        self.ux.log_warn(Component::Mesh, &line);
        self.ux.event(vrcr_shared::console::Notice::ProtocolError);
        if let Some(id) = id.filter(|i| !i.is_empty()) {
            lock(&self.inflight_cids).remove(id);
            self.complete(id, frames::fallback_result(id, fallback::PROTOCOL_ERROR));
        }
    }

    pub(super) async fn dispatch_binary(
        self: &Arc<Self>,
        link: Option<&Arc<Link>>,
        payload: &[u8],
    ) {
        let frame = match frames::decode_binary(payload) {
            Ok(f) => f,
            Err(detail) => {
                let key = format!("binary:{}", detail.split(':').next().unwrap_or_default());
                if let Some(count) = self.parse_failure_count(&key) {
                    self.ux.log_warn(
                        Component::Mesh,
                        &format!(
                            "binary frame parse failed ({key} x{count} in last min): {} bytes={}",
                            sanitize_for_console(&detail, 200),
                            payload.len()
                        ),
                    );
                }
                return;
            }
        };
        match frame {
            BinaryFrame::Resolved(resp) => {
                if resp.id.is_empty() {
                    return;
                }
                if let Some(bytes) = resp.bytes_estimate {
                    self.stats.record_bytes_estimate(bytes);
                }
                let result = MeshResult {
                    frame: json_frame(&resp),
                    action: action::RESOLVED.to_owned(),
                    reason: None,
                    retry_after_ms: None,
                };
                let url = resp.url.as_deref().filter(|u| !u.is_empty());
                self.finish_resolve(&resp.id, url, true, result);
            }
            BinaryFrame::FallbackNative(resp) => {
                if resp.id.is_empty() {
                    return;
                }
                self.log.line(&frames::fallback_native_line(
                    &resp.id,
                    resp.reason.as_deref(),
                ));
                let result = MeshResult {
                    frame: json_frame(&resp),
                    action: action::FALLBACK_NATIVE.to_owned(),
                    reason: resp.reason.clone(),
                    retry_after_ms: resp.retry_after_ms,
                };
                self.finish_resolve(&resp.id, None, false, result);
            }
            BinaryFrame::ResolveLog { id, message } => self.log.line(&format!(
                "[mesh][resolve_log] id={} {}",
                sanitize_for_console(&id, 32),
                sanitize_for_console(&message, 240)
            )),
            BinaryFrame::Pong => {
                *lock(&self.last_pong) = Some(Self::now());
                self.log
                    .line("[mesh] pong received via binary path (server should send as Text per v3.1 spec)");
            }
            BinaryFrame::Ping => {
                self.log
                    .line("[mesh] ping received via binary path (server should send as Text per v3.1 spec)");
                self.send_pong(link).await;
            }
            BinaryFrame::Other(name)
                if name == action::PROTOCOL_ERROR || name == action::RATE_LIMITED =>
            {
                self.ux.log_warn(
                    Component::Mesh,
                    &format!(
                        "{name} received via binary path (server should send as Text per v3.1 spec)"
                    ),
                );
            }
            BinaryFrame::Other(name) => self.ux.log_warn(
                Component::Mesh,
                &format!(
                    "unknown binary action -- discarding: {}",
                    sanitize_for_console(&name, 64)
                ),
            ),
        }
    }
}
