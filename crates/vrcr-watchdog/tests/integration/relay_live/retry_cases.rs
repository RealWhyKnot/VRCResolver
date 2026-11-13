use super::*;
use std::sync::atomic::{AtomicU16, AtomicU64};
use vrcr_watchdog::ipc::Feedback;

#[path = "playback_cases.rs"]
mod playback_cases;

struct FailingOrigin {
    status: AtomicU16,
    hits: AtomicUsize,
    retry_after: Option<String>,
    reason: Option<String>,
    body: Bytes,
    pause: Duration,
    broken: bool,
    direct: bool,
}

impl Upstream for FailingOrigin {
    fn send(&self, _: UpstreamRequest) -> Sending<'_> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(self.pause).await;
            let status = self.status.load(Ordering::SeqCst);
            let body = if status == 200 {
                Bytes::from_static(b"played")
            } else {
                self.body.clone()
            };
            let mut headers = HeaderMap::new();
            headers.insert("content-length", HeaderValue::from(body.len()));
            headers.insert("cache-control", HeaderValue::from_static("no-store"));
            headers.insert("content-type", HeaderValue::from_static("video/mp2t"));
            if let Some(value) = &self.retry_after {
                headers.insert("retry-after", value.parse().expect("retry"));
            }
            if let Some(value) = &self.reason {
                headers.insert("x-vrcr-reason", value.parse().expect("reason"));
            }
            let body: vrcr_watchdog::relay::upstream::BodyStream = if self.broken {
                headers.insert("content-length", HeaderValue::from(body.len() + 1));
                Box::pin(futures_util::stream::iter([
                    Ok(body),
                    Err(io::Error::other("fixture disconnected")),
                ]))
            } else {
                Box::pin(futures_util::stream::once(async { Ok(body) }))
            };
            Ok(UpstreamResponse {
                direct: self.direct,
                status: StatusCode::from_u16(status).expect("status"),
                headers,
                body,
            })
        })
    }
}

fn origin(
    status: u16,
    retry_after: Option<&str>,
    reason: Option<&str>,
    body: &'static [u8],
) -> Arc<FailingOrigin> {
    Arc::new(FailingOrigin {
        status: AtomicU16::new(status),
        hits: AtomicUsize::new(0),
        retry_after: retry_after.map(str::to_owned),
        reason: reason.map(str::to_owned),
        body: Bytes::from_static(body),
        pause: Duration::from_millis(10),
        broken: false,
        direct: true,
    })
}

fn timed_deps(origin: &Arc<FailingOrigin>, output: &Capture) -> (RelayDeps, Arc<AtomicU64>) {
    let (mut dependencies, _, _) = deps(Arc::<FailingOrigin>::clone(origin), output);
    let clock = Arc::new(AtomicU64::new(0));
    let tick = Arc::clone(&clock);
    dependencies.now = Arc::new(move || {
        SystemTime::UNIX_EPOCH + Duration::from_millis(tick.load(Ordering::SeqCst))
    });
    let request = vrcr_shared::wire::ResolveRequest {
        id: "fixture-resolve".to_owned(),
        url: "https://source.example/watch".to_owned(),
        ..Default::default()
    };
    dependencies.stats.playback.resolving(
        &request,
        SystemTime::UNIX_EPOCH - Duration::from_secs(1),
        &dependencies.ux,
    );
    dependencies
        .stats
        .playback
        .resolver_origin(&request.id, "us1.vrcresolver.com");
    for file in ["seg.ts", "index.m3u8"] {
        dependencies.stats.playback.resolved(
            &request,
            &vrcr_shared::wire::ResolveResponse {
                id: request.id.clone(),
                action: "resolved".to_owned(),
                url: Some(upstream_url("dead", file)),
                ..Default::default()
            },
            vrcr_watchdog::diagnostics::ResolveDelivery {
                cached: false,
                retried: false,
                bytes: 100,
            },
            SystemTime::UNIX_EPOCH,
            &dependencies.ux,
        );
    }
    (dependencies, clock)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_five_hundred_player_requests_use_one_upstream_call_per_window() {
    for (status, after, reason, window) in [
        (503, Some("30"), None, 30_000),
        (404, None, Some("unknown_stream"), 5_000),
    ] {
        let upstream = origin(status, after, reason, b"");
        let output = Capture::default();
        let (dependencies, clock) = timed_deps(&upstream, &output);
        dependencies
            .stats
            .playback
            .resolver_origin("resolve-retry", "us1.vrcresolver.com");
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let file = if status == 503 {
            "index.m3u8"
        } else {
            "seg.ts"
        };
        let target = upstream_url("dead", file);
        let path = play("cooldown", file, &target);
        let mut client = Conn::open(server.port()).await;
        for index in 0..500 {
            clock.store(index * (window / 500), Ordering::SeqCst);
            let reply = client.get(&path).await;
            assert_eq!(reply.status, status, "request {index}: {}", output.text());
            assert!(reply.complete);
            assert_eq!(reply.body, Vec::<u8>::new());
            if index == 0 {
                assert_eq!(reply.header("retry-after"), after);
            } else {
                let expected = (window - index * (window / 500)).div_ceil(1000).to_string();
                assert_eq!(reply.header("retry-after"), Some(expected.as_str()));
            }
            assert_eq!(reply.header("cache-control"), Some("no-store"));
        }
        assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
        clock.store(window, Ordering::SeqCst);
        assert_eq!(client.get(&path).await.status, status);
        assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);
        server.stop().await;
        let text = output.text();
        assert!(text.contains("count=501 suppressed=499"), "{text}");
        assert!(
            text.contains("suppressed=499 recovery_attempts=0 recovery=none"),
            "{text}"
        );
        assert!(!text.contains("/api/"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_suppression_reports_exact_counts_in_existing_feedback_detail() {
    let upstream = origin(503, Some("30"), None, b"");
    let output = Capture::default();
    let (mut dependencies, _) = timed_deps(&upstream, &output);
    let stats = Arc::clone(&dependencies.stats);
    let reports = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let messages = Arc::clone(&reports);
    dependencies.recovery.report = Some(Arc::new(move |feedback| {
        let context = stats.playback.context(&feedback.url).expect("context");
        let payload = vrcr_watchdog::mesh::frames::playback_feedback_payload(
            vrcr_watchdog::mesh::frames::PlaybackFeedback {
                url: &feedback.url,
                kind: feedback.kind,
                ms_since_open: feedback.ms_since_open,
                client_id: "existing",
                correlation_id: feedback.correlation_id.as_deref(),
                timestamp: SystemTime::UNIX_EPOCH,
                delivered_height: None,
                detail: feedback.detail.as_deref(),
                context: Some(&context),
            },
        );
        messages
            .lock()
            .expect("reports")
            .push(serde_json::from_str(&payload).expect("JSON"));
    }));
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let path = play("reports", "seg.ts", &upstream_url("dead", "seg.ts"));
    let mut client = Conn::open(server.port()).await;
    for _ in 0..500 {
        assert_eq!(client.get(&path).await.status, 503);
    }
    server.stop().await;
    let reports = reports.lock().expect("reports");
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["detail"], "upstream_status s=503 n=1 r=none");
    assert_eq!(reports[1]["detail"], "upstream_status s=503 n=499 r=none");
    assert_eq!(reports[1]["failure_source"], "server");
    assert_eq!(reports[1]["playback_id"], "fixture-resolve");
    assert!(
        reports[1]["request_id"]
            .as_str()
            .expect("id")
            .starts_with("relay-")
    );
    drop(reports);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_generation_burst_success_and_ranges_remain_independent() {
    let upstream = origin(502, None, None, b"generation failed");
    let output = Capture::default();
    let (dependencies, clock) = timed_deps(&upstream, &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let path = play("burst", "seg.ts", &upstream_url("dead", "seg.ts"));
    let mut client = Conn::open(server.port()).await;
    for index in 0..132 {
        clock.store(index * 38, Ordering::SeqCst);
        let reply = client.get(&path).await;
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (502, b"generation failed".as_slice())
        );
    }
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 6);
    let burst_hits = upstream.hits.load(Ordering::SeqCst);
    clock.store(7_000, Ordering::SeqCst);
    upstream.status.store(200, Ordering::SeqCst);
    assert_eq!(client.get(&path).await.body, b"played");
    upstream.status.store(503, Ordering::SeqCst);
    assert_eq!(client.get(&path).await.status, 503);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), burst_hits + 2);
    client.send("GET", &path, &[("Range", "bytes=0-99")]).await;
    assert_eq!(client.reply(false).await.status, 503);
    client
        .send("GET", &path, &[("Range", "bytes=100-199")])
        .await;
    assert_eq!(client.reply(false).await.status, 503);
    client.send("HEAD", &path, &[]).await;
    assert_eq!(client.reply(true).await.status, 503);
    assert_eq!(
        client
            .get(&play(
                "burst",
                "other.ts",
                &upstream_url("dead", "other.ts")
            ))
            .await
            .status,
        503
    );
    assert_eq!(upstream.hits.load(Ordering::SeqCst), burst_hits + 6);
    assert_eq!(client.get(&path).await.status, 503);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), burst_hits + 7);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_concurrent_requests_share_the_failure_and_retirement_is_once_per_playback() {
    for (header, body) in [
        (Some("stream_recreate_required"), b"".as_slice()),
        (None, b"stream_recreate_required".as_slice()),
        (None, br#"{"reason":"stream_recreate_required"}"#.as_slice()),
    ] {
        let upstream = origin(503, Some("30"), header, body);
        let output = Capture::default();
        let (mut dependencies, clock) = timed_deps(&upstream, &output);
        let invalidations = Arc::new(Mutex::new(Vec::<String>::new()));
        let removed = Arc::clone(&invalidations);
        dependencies.recovery.invalidate = Some(Arc::new(move |url| {
            removed.lock().expect("invalidations").push(url.to_owned())
        }));
        let reports = Arc::new(Mutex::new(Vec::<Feedback>::new()));
        let messages = Arc::clone(&reports);
        dependencies.recovery.report = Some(Arc::new(move |report| {
            messages.lock().expect("reports").push(report)
        }));
        let stats = Arc::clone(&dependencies.stats);
        let target = upstream_url("dead", "index.m3u8");
        let source = "https://source.example/watch?private=secret";
        let request = vrcr_shared::wire::ResolveRequest {
            id: "resolve-retry".to_owned(),
            url: source.to_owned(),
            ..Default::default()
        };
        stats
            .playback
            .resolving(&request, SystemTime::UNIX_EPOCH, &dependencies.ux);
        stats.playback.resolved(
            &request,
            &vrcr_shared::wire::ResolveResponse {
                id: request.id.clone(),
                action: "resolved".to_owned(),
                url: Some(target.clone()),
                ..Default::default()
            },
            vrcr_watchdog::diagnostics::ResolveDelivery {
                cached: false,
                retried: false,
                bytes: 100,
            },
            SystemTime::UNIX_EPOCH,
            &dependencies.ux,
        );
        dependencies
            .stats
            .playback
            .resolver_origin("resolve-retry", "us1.vrcresolver.com");
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..20 {
            let path = play("retired", "index.m3u8", &target);
            let port = server.port();
            tasks.spawn(async move { Conn::open(port).await.get(&path).await });
        }
        while let Some(reply) = tasks.join_next().await {
            let reply = reply.expect("task");
            assert_eq!(reply.status, 410);
            assert_eq!(reply.header("connection"), Some("close"));
            assert_eq!(reply.body, b"stream_recreate_required");
            assert!(reply.complete);
        }
        let initial_hits = upstream.hits.load(Ordering::SeqCst);
        assert!((1..=20).contains(&initial_hits));
        assert_eq!(
            invalidations
                .lock()
                .expect("invalidations")
                .iter()
                .filter(|url| url.as_str() == source)
                .count(),
            1
        );
        let before = upstream.hits.load(Ordering::SeqCst);
        clock.store(60_001, Ordering::SeqCst);
        assert_eq!(
            Conn::open(server.port())
                .await
                .get(&play(
                    "retired",
                    "other.ts",
                    &upstream_url("dead", "other.ts")
                ))
                .await
                .status,
            410
        );
        assert_eq!(upstream.hits.load(Ordering::SeqCst), before);
        let context = stats.playback.context(source).expect("context");
        assert_eq!(context.recovery_attempts, 1);
        assert_eq!(context.recovery_result, "clean_failure");
        assert_eq!(
            context.suppressed_requests,
            21 - u64::try_from(initial_hits).expect("hits")
        );
        assert_eq!(context.retry_reason, "stream_recreate_required");
        server.stop().await;
        assert!(!reports.lock().expect("reports").is_empty());
        assert!(
            output
                .text()
                .contains("recovery_attempts=1 recovery=clean_failure")
        );
        assert!(!output.text().contains("private"));
        assert!(!output.text().contains("secret"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_oversized_and_broken_errors_are_forwarded_without_caching_a_partial_body() {
    for broken in [false, true] {
        let mut upstream = origin(503, Some("30"), None, b"incomplete");
        let unique = Arc::get_mut(&mut upstream).expect("unique");
        unique.broken = broken;
        if !broken {
            unique.body = Bytes::from(vec![b'x'; 16_384]);
        }
        let output = Capture::default();
        let (dependencies, _) = timed_deps(&upstream, &output);
        dependencies
            .stats
            .playback
            .resolver_origin("resolve-retry", "us1.vrcresolver.com");
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let path = play("large", "seg.ts", &upstream_url("dead", "seg.ts"));
        for _ in 0..2 {
            let reply = Conn::open(server.port()).await.get(&path).await;
            assert_eq!(reply.status, 503);
            assert_eq!(reply.complete, !broken);
            if !broken {
                assert_eq!(reply.body, upstream.body);
            }
        }
        assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);
        server.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_slow_healthy_headers_do_not_serialize_repeated_requests() {
    let mut upstream = origin(200, None, None, b"");
    Arc::get_mut(&mut upstream).expect("unique").pause = Duration::from_secs(3);
    let output = Capture::default();
    let (dependencies, _) = timed_deps(&upstream, &output);
    let mut config = RelayConfig::http(0);
    config.timeout = Some(Duration::from_secs(4));
    let server = RelayServer::start(config, dependencies)
        .await
        .expect("relay");
    let path = play("healthy", "seg.ts", &upstream_url("dead", "seg.ts"));
    let port = server.port();
    let first_path = path.clone();
    let first = tokio::spawn(async move { Conn::open(port).await.get(&first_path).await });
    assert!(
        eventually(
            || upstream.hits.load(Ordering::SeqCst) == 1,
            Duration::from_secs(1)
        )
        .await
    );
    let second = tokio::spawn(async move { Conn::open(port).await.get(&path).await });
    assert!(
        eventually(
            || upstream.hits.load(Ordering::SeqCst) == 2,
            Duration::from_secs(1)
        )
        .await
    );
    for reply in [first.await.expect("first"), second.await.expect("second")] {
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"played");
        assert!(reply.complete);
    }
    assert!(!output.text().contains("retry_wait_timeout"));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_untrusted_recreation_empty_ids_and_fresh_playbacks() {
    for (direct, configured) in [(false, "us1.vrcresolver.com"), (true, "different.example")] {
        let mut upstream = origin(503, Some("30"), Some("stream_recreate_required"), b"");
        Arc::get_mut(&mut upstream).expect("unique").direct = direct;
        let output = Capture::default();
        let (dependencies, _) = timed_deps(&upstream, &output);
        dependencies
            .stats
            .playback
            .resolver_origin("fixture-resolve", configured);
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let path = play("untrusted", "seg.ts", &upstream_url("dead", "seg.ts"));
        assert_eq!(Conn::open(server.port()).await.get(&path).await.status, 503);
        assert!(!output.text().contains("recovery=clean_failure"));
        assert_eq!(
            Conn::open(server.port()).await.get("/play//x").await.status,
            404
        );
        assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
        server.stop().await;
    }
    let upstream = origin(503, Some("30"), Some("stream_recreate_required"), b"");
    let output = Capture::default();
    let (dependencies, clock) = timed_deps(&upstream, &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let target = upstream_url("dead", "index.m3u8");
    let policy = FirstPartyPolicy::default();
    let first =
        vrcr_shared::trust_gateway::build(&policy, u32::from(server.port()), &target, None, "http")
            .expect("first URL");
    let second =
        vrcr_shared::trust_gateway::build(&policy, u32::from(server.port()), &target, None, "http")
            .expect("second URL");
    let first = url::Url::parse(&first).expect("first");
    let second = url::Url::parse(&second).expect("second");
    assert_ne!(
        vrcr_watchdog::relay::activity::play_id(first.path()),
        vrcr_watchdog::relay::activity::play_id(second.path())
    );
    let old = format!("{}?{}", first.path(), first.query().expect("query"));
    let new = format!("{}?{}", second.path(), second.query().expect("query"));
    assert_eq!(Conn::open(server.port()).await.get(&old).await.status, 410);
    upstream.status.store(200, Ordering::SeqCst);
    assert_eq!(Conn::open(server.port()).await.get(&new).await.status, 200);
    for tick in [60_000, 120_000, 240_000, 299_999] {
        clock.store(tick, Ordering::SeqCst);
        assert_eq!(Conn::open(server.port()).await.get(&old).await.status, 410);
    }
    clock.store(300_000, Ordering::SeqCst);
    assert_eq!(Conn::open(server.port()).await.get(&old).await.status, 200);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_one_untyped_failure_does_not_delay_the_next_success() {
    let upstream = origin(503, None, None, b"");
    let output = Capture::default();
    let (dependencies, _) = timed_deps(&upstream, &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let path = play("single", "seg.ts", &upstream_url("dead", "seg.ts"));
    let mut client = Conn::open(server.port()).await;
    assert_eq!(client.get(&path).await.status, 503);
    upstream.status.store(200, Ordering::SeqCst);
    assert_eq!(client.get(&path).await.status, 200);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);
    server.stop().await;
}
