use super::*;

impl Inner {
    pub(super) async fn wait_for_link(
        &self,
        reply: &mut oneshot::Receiver<MeshResult>,
    ) -> LinkWait {
        let mut woke = false;
        loop {
            let mut next = self.link_ready.subscribe();
            if let Some(link) = self.ready_link() {
                return LinkWait::Ready(link);
            }
            if !woke {
                self.wake_reconnect_loop();
                woke = true;
            }
            let mut settled = self.connect_settled.subscribe();
            let grace_end = *lock(&self.link_down_since) + self.cfg.reconnect_grace;
            let grace_ran_out = grace_end <= Self::now();
            if grace_ran_out && !self.connect_in_flight.load(Ordering::SeqCst) {
                return LinkWait::Gone;
            }
            tokio::select! {
                _ = next.changed() => {
                    woke = false;
                }
                r = &mut *reply => {
                    return match r {
                        Ok(m) => LinkWait::Reply(m),
                        Err(_) => LinkWait::Gone,
                    };
                }
                () = sleep_until(grace_end), if !grace_ran_out => {}
                _ = settled.changed(), if grace_ran_out => {
                    return match self.ready_link() {
                        Some(link) => LinkWait::Ready(link),
                        None => LinkWait::Gone,
                    };
                }
            }
        }
    }

    pub(super) async fn wait_for_reply(
        self: &Arc<Self>,
        link: &Arc<Link>,
        reply: &mut oneshot::Receiver<MeshResult>,
    ) -> ReplyWait {
        loop {
            let window_start = Self::now();
            tokio::select! {
                r = &mut *reply => {
                    return match r {
                        Ok(m) => ReplyWait::Got(m),
                        Err(_) => ReplyWait::Dropped,
                    };
                }
                () = link.wait_down() => return ReplyWait::Dropped,
                () = sleep(self.cfg.stall_probe_after) => self.probe_if_quiet(link, window_start),
            }
        }
    }

    pub(super) async fn resolve(
        self: Arc<Self>,
        mut req: ResolveRequest,
        deadline: Instant,
    ) -> MeshResult {
        if req.id.is_empty() {
            req.id = identity::fresh_id();
        }
        let id = req.id.clone();
        if lock(&self.rate_limited_until).is_some_and(|until| until > Self::now()) {
            return frames::fallback_result(&id, fallback::RATE_LIMITED);
        }
        let (tx, mut rx) = oneshot::channel();
        lock(&self.pending).insert(id.clone(), tx);
        let cid = req
            .correlation_id
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| id.clone());
        lock(&self.inflight_cids).insert(id.clone(), cid);
        let _guard = PendingGuard {
            inner: &self,
            id: id.clone(),
        };
        match timeout_at(deadline, self.resolve_loop(&mut req, &mut rx, deadline)).await {
            Ok(result) => result,
            Err(_) => frames::fallback_result(&id, fallback::CLIENT_DEADLINE_EXCEEDED),
        }
    }

    pub(super) async fn resolve_loop(
        self: &Arc<Self>,
        req: &mut ResolveRequest,
        rx: &mut oneshot::Receiver<MeshResult>,
        deadline: Instant,
    ) -> MeshResult {
        let id = req.id.clone();
        let cid = cid_suffix(req.correlation_id.as_deref());
        let mut sends = 0u32;
        let mut drops = 0u32;
        loop {
            let link = match self.wait_for_link(rx).await {
                LinkWait::Reply(m) => return m,
                LinkWait::Gone => {
                    self.forget_pending(&id);
                    if sends > 0 {
                        self.log.line(&format!(
                            "[mesh] no reconnect within {}s; failing id={id}{cid}",
                            secs(self.cfg.reconnect_grace)
                        ));
                    }
                    return frames::fallback_result(&id, fallback::SERVER_UNREACHABLE);
                }
                LinkWait::Ready(link) => link,
            };
            if let Ok(m) = rx.try_recv() {
                return m;
            }
            if lock(&self.server).protocol_version >= 2
                && req.protocol_version.is_none()
                && caller_opted_into_v2(req)
            {
                req.protocol_version = Some(wire::CLIENT_PROTOCOL_VERSION);
            }
            req.wrapper_deadline_ms = Some(frames::wire_deadline_for(deadline, Self::now()));
            let payload = match serde_json::to_string(&*req) {
                Ok(p) => p,
                Err(e) => {
                    self.forget_pending(&id);
                    self.ux.warn(
                        Component::Mesh,
                        &format!("request serialization failed id={id}: {e}"),
                    );
                    return frames::fallback_result(&id, fallback::INTERNAL_ERROR);
                }
            };
            let sent = match self.send_text(&link, payload).await {
                Ok(s) => s,
                Err(e) => {
                    self.ux.warn(
                        Component::Mesh,
                        &format!(
                            "send failed id={id}{cid}: {}",
                            crate::diagnostics::error_kind(&e)
                        ),
                    );
                    false
                }
            };
            if !sent {
                let link_dropped = tokio::select! {
                    () = link.wait_down() => true,
                    r = &mut *rx => {
                        return r.unwrap_or_else(|_| frames::fallback_result(&id, fallback::SERVER_UNREACHABLE));
                    }
                    () = sleep(self.cfg.reconnect_grace) => {
                        self.forget_pending(&id);
                        return frames::fallback_result(&id, fallback::SERVER_UNREACHABLE);
                    }
                };
                if link_dropped && sends > 0 {
                    drops += 1;
                    if drops > 1 {
                        return self.give_up_after_resend(&id, &cid);
                    }
                }
                continue;
            }
            sends += 1;
            if sends > 1 {
                self.log.line(&format!(
                    "[mesh] resent id={id}{cid} after reconnect (send {sends})"
                ));
            }
            match self.wait_for_reply(&link, rx).await {
                ReplyWait::Got(m) => return m,
                ReplyWait::Dropped => {
                    if let Ok(m) = rx.try_recv() {
                        return m;
                    }
                }
            }
            drops += 1;
            if drops > 1 {
                return self.give_up_after_resend(&id, &cid);
            }
            self.log.line(&format!(
                "[mesh] connection dropped with id={id}{cid} pending; resending after reconnect"
            ));
        }
    }

    pub(super) fn give_up_after_resend(&self, id: &str, cid: &str) -> MeshResult {
        self.forget_pending(id);
        self.log.line(&format!(
            "[mesh] connection dropped again with id={id}{cid} pending; not resending a second time"
        ));
        frames::fallback_result(id, fallback::SERVER_UNREACHABLE)
    }

    pub(super) fn fail_all_pending(&self, reason: &str) {
        lock(&self.inflight_cids).clear();
        let mut drained: Vec<(String, oneshot::Sender<MeshResult>)> =
            lock(&self.pending).drain().collect();
        if drained.is_empty() {
            return;
        }
        drained.sort_by(|a, b| a.0.cmp(&b.0));
        let ids: Vec<&str> = drained.iter().map(|(id, _)| id.as_str()).collect();
        let list = if ids.len() <= MAX_IDS_IN_LINE {
            ids.join(",")
        } else {
            format!(
                "{},...(+{})",
                ids[..MAX_IDS_IN_LINE].join(","),
                ids.len() - MAX_IDS_IN_LINE
            )
        };
        self.ux.warn(
            Component::Mesh,
            &format!(
                "failing {} pending requests reason={reason} ids={list}",
                drained.len()
            ),
        );
        for (id, tx) in drained {
            let _ = tx.send(frames::fallback_result(&id, reason));
        }
    }

    pub(super) fn complete(&self, id: &str, result: MeshResult) -> bool {
        let Some(tx) = lock(&self.pending).remove(id) else {
            return false;
        };
        let _ = tx.send(result);
        true
    }

    pub(super) fn lookup_recent_cid(&self, url: &str) -> Option<String> {
        let mut recent = lock(&self.recent_cids);
        let (cid, at) = recent.get(url)?.clone();
        if at.elapsed() > RECENT_CIDS_TTL {
            recent.remove(url);
            return None;
        }
        drop(recent);
        Some(cid)
    }

    pub(super) fn remember_resolved_url_cid(&self, url: &str, cid: &str) {
        if url.is_empty() || cid.is_empty() {
            return;
        }
        let mut recent = lock(&self.recent_cids);
        recent.insert(url.to_owned(), (cid.to_owned(), Self::now()));
        if recent.len() <= MAX_RECENT_CIDS {
            return;
        }
        let oldest = recent
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            recent.remove(&k);
        }
    }
}
