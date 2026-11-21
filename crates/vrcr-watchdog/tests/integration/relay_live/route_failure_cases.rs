use super::*;
use vrcr_watchdog::relay::retry::RouteFailure;

fn recording(dependencies: &mut RelayDeps) -> Arc<Mutex<Vec<RouteFailure>>> {
    let failures = Arc::new(Mutex::new(Vec::<RouteFailure>::new()));
    let seen = Arc::clone(&failures);
    dependencies.recovery.failed = Some(Arc::new(move |failure: &RouteFailure| {
        seen.lock().expect("failures").push(failure.clone());
    }));
    failures
}

async fn first_failure(failures: &Arc<Mutex<Vec<RouteFailure>>>) -> RouteFailure {
    let recorded = Arc::clone(failures);
    assert!(
        eventually(|| !recorded.lock().expect("failures").is_empty(), WITHIN).await,
        "no route failure was reported"
    );
    failures.lock().expect("failures")[0].clone()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vrcdn_style_401_fails_the_route_once_and_arms_the_failure_hint() {
    let upstream = origin(401, None, None, b"");
    let output = Capture::default();
    let (mut dependencies, _) = timed_deps(&upstream, &output);
    let hint = Arc::new(vrcr_watchdog::og_hint::OgFallbackHint::default());
    let failures = Arc::new(Mutex::new(Vec::<RouteFailure>::new()));
    let seen = Arc::clone(&failures);
    let armed = Arc::clone(&hint);
    dependencies.recovery.failed = Some(Arc::new(move |failure: &RouteFailure| {
        let source = failure
            .source_url
            .clone()
            .unwrap_or_else(|| failure.target_url.clone());
        vrcr_watchdog::og_hint::arm(None, Some(&armed), &source);
        seen.lock().expect("failures").push(failure.clone());
    }));
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let path = play("vrcdn", "seg.ts", &upstream_url("dead", "seg.ts"));
    let mut client = Conn::open(server.port()).await;
    for _ in 0..3 {
        assert_eq!(client.get(&path).await.status, 401);
    }
    let recorded = Arc::clone(&failures);
    assert!(eventually(|| !recorded.lock().expect("failures").is_empty(), WITHIN).await);
    server.stop().await;
    let failures = failures.lock().expect("failures").clone();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].reason, "http_401");
    assert_eq!(
        failures[0].source_url.as_deref(),
        Some("https://source.example/watch")
    );
    assert!(hint.should_prefer_og("https://source.example/watch"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_healthy_route_never_reports_a_route_failure() {
    let upstream = origin(200, None, None, b"");
    let output = Capture::default();
    let (mut dependencies, _) = timed_deps(&upstream, &output);
    let failures = Arc::new(Mutex::new(0usize));
    let seen = Arc::clone(&failures);
    dependencies.recovery.failed = Some(Arc::new(move |_: &RouteFailure| {
        *seen.lock().expect("failures") += 1;
    }));
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let path = play("healthy", "seg.ts", &upstream_url("dead", "seg.ts"));
    let mut client = Conn::open(server.port()).await;
    for _ in 0..3 {
        assert_eq!(client.get(&path).await.status, 200);
    }
    server.stop().await;
    assert_eq!(*failures.lock().expect("failures"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_segment_error_after_healthy_segments_does_not_fail_the_route() {
    for status in [404, 502] {
        let upstream = origin(200, None, None, b"");
        let output = Capture::default();
        let (mut dependencies, _) = timed_deps(&upstream, &output);
        let failures = recording(&mut dependencies);
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let segment = play("healthy-live", "seg.ts", &upstream_url("dead", "seg.ts"));
        let manifest = play(
            "healthy-live",
            "index.m3u8",
            &upstream_url("dead", "index.m3u8"),
        );
        let mut client = Conn::open(server.port()).await;
        for _ in 0..2 {
            assert_eq!(client.get(&segment).await.status, 200);
        }
        upstream.status.store(status, Ordering::SeqCst);
        assert_eq!(client.get(&segment).await.status, status);
        assert_eq!(client.get(&manifest).await.status, status);
        let failure = first_failure(&failures).await;
        server.stop().await;
        assert_eq!(failures.lock().expect("failures").len(), 1);
        assert!(failure.target_url.ends_with("index.m3u8"), "{failure:?}");
        assert_eq!(failure.reason, format!("http_{status}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_or_manifest_error_fails_the_route_and_arms_the_hint() {
    for (status, file) in [
        (401, "index.m3u8"),
        (404, "index.m3u8"),
        (502, "index.m3u8"),
        (404, "seg.ts"),
    ] {
        let upstream = origin(status, None, None, b"");
        let output = Capture::default();
        let (mut dependencies, _) = timed_deps(&upstream, &output);
        let hint = Arc::new(vrcr_watchdog::og_hint::OgFallbackHint::default());
        let armed = Arc::clone(&hint);
        let failures = Arc::new(Mutex::new(Vec::<RouteFailure>::new()));
        let seen = Arc::clone(&failures);
        dependencies.recovery.failed = Some(Arc::new(move |failure: &RouteFailure| {
            let source = failure.source_url.clone().unwrap_or_default();
            vrcr_watchdog::og_hint::arm(None, Some(&armed), &source);
            seen.lock().expect("failures").push(failure.clone());
        }));
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let path = play("first", file, &upstream_url("dead", file));
        let mut client = Conn::open(server.port()).await;
        assert_eq!(client.get(&path).await.status, status);
        let failure = first_failure(&failures).await;
        server.stop().await;
        assert_eq!(failure.reason, format!("http_{status}"), "{file}");
        assert!(
            hint.should_prefer_og("https://source.example/watch"),
            "{status} {file}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_requests_without_a_target_never_fail_a_route() {
    let upstream = origin(200, None, None, b"");
    let output = Capture::default();
    let (mut dependencies, _) = timed_deps(&upstream, &output);
    let failures = recording(&mut dependencies);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let mut client = Conn::open(server.port()).await;
    let refused = play("no-target", "index.m3u8", "http://127.0.0.1/index.m3u8");
    assert_eq!(client.get(&refused).await.status, 403);
    upstream.status.store(404, Ordering::SeqCst);
    let manifest = play(
        "no-target",
        "index.m3u8",
        &upstream_url("dead", "index.m3u8"),
    );
    assert_eq!(client.get(&manifest).await.status, 404);
    let failure = first_failure(&failures).await;
    server.stop().await;
    assert_eq!(failures.lock().expect("failures").len(), 1);
    assert!(!failure.target_url.is_empty(), "{failure:?}");
}
