use super::*;

impl Inner {
    pub(super) async fn run(self: Arc<Self>) {
        let mut shutdown = self.shutdown.subscribe();
        loop {
            if *shutdown.borrow() {
                break;
            }
            *lock(&self.last_attempt_start) = Self::now();
            let link = match self.attempt(&mut shutdown).await {
                Attempt::Cancelled => break,
                Attempt::Ended(link) => link,
            };
            if *shutdown.borrow() {
                break;
            }
            let stayed_up = link
                .as_ref()
                .is_some_and(|l| l.was_ready.load(Ordering::SeqCst))
                && lock(&self.link_ready_at)
                    .is_some_and(|t| t.elapsed() >= self.cfg.stable_link_after);
            if stayed_up {
                self.reconnect_attempt.store(0, Ordering::SeqCst);
            }
            let attempt = self.reconnect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
            self.stats.record_reconnect();
            let wait_ms = (self.cfg.next_backoff_ms)(attempt);
            if attempt <= 5 || attempt.is_multiple_of(10) {
                self.ux.write(
                    Component::Mesh,
                    &format!("reconnect attempt {attempt} in {} s", wait_ms / 1000),
                );
            }
            if !self.wait_before_reconnect(wait_ms, &mut shutdown).await {
                break;
            }
        }
    }

    pub(super) async fn wait_before_reconnect(
        &self,
        wait_ms: u64,
        shutdown: &mut watch::Receiver<bool>,
    ) -> bool {
        let wait = Duration::from_millis(wait_ms);
        let backoff_end = Self::now() + wait;
        let mut wake = self.wake.subscribe();
        let woke = tokio::select! {
            () = sleep(wait) => false,
            _ = wake.wait_for(|w| *w) => true,
            _ = shutdown.wait_for(|s| *s) => return false,
        };
        if woke {
            let wake_at =
                (*lock(&self.last_attempt_start) + self.cfg.min_wake_spacing).min(backoff_end);
            tokio::select! {
                () = sleep_until(wake_at) => {}
                _ = shutdown.wait_for(|s| *s) => return false,
            }
        }
        if *shutdown.borrow() {
            return false;
        }
        self.wake.send_replace(false);
        true
    }

    pub(super) fn wake_reconnect_loop(&self) {
        self.wake.send_replace(true);
    }

    pub(super) fn settle_connect(&self) {
        self.connect_in_flight.store(false, Ordering::SeqCst);
        self.connect_settled.send_modify(|g| *g = g.wrapping_add(1));
    }

    pub(super) fn prepare_welcome_state(&self, node: &str) {
        *lock(&self.server) = Server {
            negotiated_format: format::JSON.to_owned(),
            current_host: node.to_owned(),
            ..Server::default()
        };
    }

    pub(super) fn report_failure(&self, text: &str) {
        let attempt = self.reconnect_attempt.load(Ordering::SeqCst);
        if self.was_connected.load(Ordering::SeqCst) || attempt == 0 {
            self.ux.warn(Component::Mesh, text);
        } else {
            self.log.line(&format!(
                "[mesh] reconnect attempt {attempt} failed: {text}"
            ));
        }
        self.was_connected.store(false, Ordering::SeqCst);
    }

    pub(super) async fn attempt(self: &Arc<Self>, shutdown: &mut watch::Receiver<bool>) -> Attempt {
        let node = self.cfg.proxy_host.clone();
        self.prepare_welcome_state(&node);
        let url = (self.cfg.url_for_host)(&node);
        self.connect_in_flight.store(true, Ordering::SeqCst);
        let mut probe = Probe::default();
        let started = Self::now();
        let outcome = tokio::select! {
            r = timeout(
                self.cfg.connect_deadline,
                connect::open(&url, &self.cfg.user_agent, || self.tls_config(), &mut probe),
            ) => Some(r),
            _ = shutdown.wait_for(|s| *s) => None,
        };
        *lock(&self.last_probe) = probe;
        let elapsed_ms = started.elapsed().as_millis();
        let opened = match outcome {
            None => {
                self.settle_connect();
                return Attempt::Cancelled;
            }
            Some(Ok(Ok(opened))) => opened,
            Some(Ok(Err(e))) => {
                let text = frames::describe_connect_failure(
                    &node,
                    elapsed_ms,
                    &probe,
                    Some(&e.describe()),
                );
                self.report_failure(&text);
                self.settle_connect();
                return Attempt::Ended(None);
            }
            Some(Err(_)) => {
                let text = frames::describe_connect_failure(&node, elapsed_ms, &probe, None);
                self.report_failure(&text);
                self.settle_connect();
                return Attempt::Ended(None);
            }
        };

        self.log.line(&format!(
            "[mesh] connected in {elapsed_ms} ms{}",
            probe.timings()
        ));
        let is_v3 = should_send_client_hello(opened.subprotocol.as_deref());
        lock(&self.server).is_v3 = is_v3;
        self.log.line(&format!(
            "[mesh][v3] negotiated subprotocol={} v3={is_v3} deflate-offered=false",
            opened.subprotocol.as_deref().unwrap_or("<none>")
        ));

        let (sink, stream) = opened.socket.split();
        let link = Arc::new(Link::new(sink));
        *lock(&self.current) = Some(Arc::clone(&link));
        if is_v3 && let Err(e) = self.send_client_hello(&link, &node).await {
            self.log.line(&format!(
                "[mesh][v3] client_hello send failed: {}",
                sanitize_for_console(&e, 160)
            ));
        }
        self.was_connected.store(true, Ordering::SeqCst);
        *lock(&self.last_pong) = Some(Self::now());
        self.arm_welcome(&link);
        self.ux
            .write(Component::Mesh, &format!("connected node={node}"));

        let pumped = tokio::select! {
            r = self.pump(&link, stream) => Some(r),
            _ = shutdown.wait_for(|s| *s) => None,
        };
        match &pumped {
            Some(Ok(())) => self.ux.write(Component::Mesh, "disconnected (clean close)"),
            Some(Err(e)) => self.report_failure(&format!(
                "disconnected (error): {}",
                sanitize_for_console(e, 160)
            )),
            None => {}
        }
        self.mark_link_down(&link);
        self.settle_connect();
        link.settle_welcome();
        link.open.store(false, Ordering::SeqCst);
        {
            let mut current = lock(&self.current);
            if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &link)) {
                *current = None;
            }
        }
        match pumped {
            None => Attempt::Cancelled,
            Some(_) => Attempt::Ended(Some(link)),
        }
    }

    pub(super) async fn send_client_hello(&self, link: &Link, node: &str) -> Result<(), String> {
        let hash = self.welcome_cache.get(node).and_then(|e| e.welcome_hash);
        let hello = ClientHelloFrame {
            action: action::CLIENT_HELLO.to_owned(),
            welcome_hash: hash.clone(),
            client_id: self.client_id.clone(),
            accept_formats: Some(format::PREFERENCE.iter().map(|f| (*f).to_owned()).collect()),
            ..Default::default()
        };
        let text =
            serde_json::to_string(&hello).map_err(|e| format!("serialize client hello: {e}"))?;
        self.send_text(link, text).await?;
        self.log.line(&format!(
            "[mesh][v3] client_hello sent node={node} hash={}",
            hash.as_deref().unwrap_or("null")
        ));
        Ok(())
    }

    pub(super) fn arm_welcome(self: &Arc<Self>, link: &Arc<Link>) {
        let inner = Arc::clone(self);
        let link = Arc::clone(link);
        let wait = self.cfg.welcome_timeout;
        tokio::spawn(async move {
            let mut rx = link.welcome.subscribe();
            let settled = tokio::select! {
                _ = rx.wait_for(|w| *w) => true,
                () = sleep(wait) => false,
            };
            if !settled && link.settle_welcome() {
                {
                    let mut s = lock(&inner.server);
                    if s.protocol_version == 0 {
                        s.protocol_version = 1;
                    }
                }
                inner
                    .log
                    .line("[mesh] no welcome within 1s -- assuming v1 server");
            }
            inner.mark_link_ready(&link);
        });
    }

    pub(super) fn mark_link_ready(&self, link: &Arc<Link>) {
        if !link.is_open() {
            return;
        }
        *lock(&self.link_ready_at) = Some(Self::now());
        link.was_ready.store(true, Ordering::SeqCst);
        self.wake.send_replace(false);
        *lock(&self.ready) = Some(Arc::clone(link));
        self.link_ready.send_modify(|g| *g = g.wrapping_add(1));
        self.settle_connect();
    }

    pub(super) fn mark_link_down(&self, link: &Arc<Link>) {
        {
            let mut ready = lock(&self.ready);
            if ready.as_ref().is_some_and(|r| Arc::ptr_eq(r, link)) {
                *ready = None;
            }
        }
        if link.was_ready.load(Ordering::SeqCst) {
            *lock(&self.link_down_since) = Self::now();
        }
        link.down.send_replace(true);
    }

    pub(super) async fn send_text(&self, link: &Link, text: String) -> Result<bool, String> {
        if !link.is_open() {
            return Ok(false);
        }
        let deadline = Self::now() + self.cfg.send_deadline;
        let Ok(mut sink) = timeout_at(deadline, link.sink.lock()).await else {
            self.log.line(&format!(
                "[mesh] send skipped: another send has been stuck for over {}s",
                secs(self.cfg.send_deadline)
            ));
            return Ok(false);
        };
        if !link.is_open() {
            return Ok(false);
        }
        match timeout_at(deadline, sink.send(Message::text(text))).await {
            Ok(Ok(())) => Ok(true),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => {
                self.ux.warn(
                    Component::Mesh,
                    &format!(
                        "send stalled for {}s; dropping the connection to reconnect",
                        secs(self.cfg.send_deadline)
                    ),
                );
                link.abort();
                Ok(false)
            }
        }
    }

    pub(super) async fn pump(
        self: &Arc<Self>,
        link: &Arc<Link>,
        mut stream: SplitStream<Socket>,
    ) -> Result<(), String> {
        let _heartbeat = AbortOnDrop(tokio::spawn(Arc::clone(self).heartbeat(Arc::clone(link))));
        let mut abort = link.abort.subscribe();
        loop {
            let next = tokio::select! {
                _ = abort.wait_for(|a| *a) => return Err("connection aborted".to_owned()),
                m = stream.next() => m,
            };
            let msg = match next {
                None => return Err("connection closed without a close frame".to_owned()),
                Some(Err(e)) => return Err(e.to_string()),
                Some(Ok(m)) => m,
            };
            *lock(&self.last_inbound) = Some(Self::now());
            match msg {
                Message::Text(t) => self.dispatch_text(Some(link), t.as_str().as_bytes()).await,
                Message::Binary(b) => {
                    if self.is_msgpack() {
                        self.dispatch_binary(Some(link), &b).await;
                    } else {
                        self.ux.warn(
                            Component::Mesh,
                            "unexpected Binary frame on json-negotiated connection -- aborting + reconnecting",
                        );
                        link.abort();
                        return Ok(());
                    }
                }
                Message::Close(_) => return Ok(()),
                _ => {}
            }
        }
    }

    pub(super) async fn heartbeat(self: Arc<Self>, link: Arc<Link>) {
        loop {
            sleep(self.cfg.heartbeat).await;
            if !link.is_open() {
                return;
            }
            let sent_at = Self::now();
            if self
                .send_text(&link, frames::PING_FRAME.to_owned())
                .await
                .is_err()
            {
                link.abort();
                return;
            }
            sleep(self.cfg.pong_deadline).await;
            let last_pong = *lock(&self.last_pong);
            let Some(pong) = last_pong.filter(|p| *p >= sent_at) else {
                link.abort();
                return;
            };
            self.stats.perf.record_mesh_rtt(pong - sent_at);
        }
    }

    pub(super) fn probe_if_quiet(self: &Arc<Self>, link: &Arc<Link>, quiet_since: Instant) {
        if lock(&self.last_inbound).is_some_and(|t| t >= quiet_since) || link.is_down() {
            return;
        }
        if link.probing.swap(true, Ordering::SeqCst) {
            return;
        }
        {
            let mut last = lock(&link.last_probe);
            if last.is_some_and(|t| t >= quiet_since) {
                link.probing.store(false, Ordering::SeqCst);
                return;
            }
            *last = Some(Self::now());
        }
        tokio::spawn(Arc::clone(self).probe_link(Arc::clone(link)));
    }

    pub(super) async fn probe_link(self: Arc<Self>, link: Arc<Link>) {
        let probe_at = Self::now();
        match self.send_text(&link, frames::PING_FRAME.to_owned()).await {
            Err(_) => link.abort(),
            Ok(false) => {}
            Ok(true) => {
                tokio::select! {
                    () = sleep(self.cfg.stall_pong_deadline) => {}
                    () = link.wait_down() => {}
                }
                let answered = lock(&self.last_inbound).is_some_and(|t| t >= probe_at);
                if !link.is_down() && !answered {
                    self.log.line(&format!(
                        "[mesh] no answer to a ping within {}s while a resolve waited; dropping the connection to reconnect",
                        secs(self.cfg.stall_pong_deadline)
                    ));
                    link.abort();
                }
            }
        }
        link.probing.store(false, Ordering::SeqCst);
    }
}
