use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_transport_failures_print_once_until_recovery() {
    struct Transport {
        kind: &'static str,
        healthy: std::sync::atomic::AtomicBool,
    }
    impl Upstream for Transport {
        fn send(&self, _: UpstreamRequest) -> Sending<'_> {
            Box::pin(async move {
                if self.healthy.load(Ordering::SeqCst) {
                    return Ok(UpstreamResponse {
                        direct: true,
                        status: StatusCode::OK,
                        headers: HeaderMap::new(),
                        body: Box::pin(futures_util::stream::empty()),
                    });
                }
                if self.kind == "deadline" {
                    return std::future::pending().await;
                }
                Err(UpstreamError::new(self.kind, "private transport details"))
            })
        }
    }
    for (kind, sentence, status) in [
        ("connect_error", "Couldn't reach the server.", 502),
        ("request_error", "A request to the server failed.", 502),
        ("timeout", "The server took too long to answer.", 502),
        ("deadline", "The server took too long to answer.", 504),
    ] {
        let upstream = Arc::new(Transport {
            kind,
            healthy: std::sync::atomic::AtomicBool::new(false),
        });
        let output = Capture::default();
        let (dependencies, _, _) = deps(Arc::<Transport>::clone(&upstream), &output);
        let mut config = RelayConfig::http(0);
        config.timeout = Some(Duration::from_millis(30));
        let server = RelayServer::start(config, dependencies)
            .await
            .expect("relay");
        let mut client = Conn::open(server.port()).await;
        let path = play("transport", "seg.ts", &upstream_url("transport", "seg.ts"));
        for _ in 0..3 {
            assert_eq!(client.get(&path).await.status, status);
        }
        assert_eq!(output.text().matches(sentence).count(), 1, "{kind}");
        upstream.healthy.store(true, Ordering::SeqCst);
        assert_eq!(client.get(&path).await.status, 200);
        assert!(eventually(|| server.in_flight() == 0, WITHIN).await);
        upstream.healthy.store(false, Ordering::SeqCst);
        assert_eq!(client.get(&path).await.status, status);
        assert_eq!(output.text().matches(sentence).count(), 2, "{kind}");
        assert!(!output.text().contains("private transport details"));
        assert!(!output.text().contains("[hitch]"));
        server.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_success_clears_a_failure_installed_by_an_overlapping_request() {
    struct Racing {
        hits: AtomicUsize,
        release: tokio::sync::Notify,
    }
    impl Upstream for Racing {
        fn send(&self, _: UpstreamRequest) -> Sending<'_> {
            let index = self.hits.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if index == 0 {
                    self.release.notified().await;
                }
                let mut headers = HeaderMap::new();
                headers.insert("content-length", HeaderValue::from_static("0"));
                headers.insert("retry-after", HeaderValue::from_static("30"));
                Ok(UpstreamResponse {
                    direct: true,
                    status: if index == 1 {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::OK
                    },
                    headers,
                    body: Box::pin(futures_util::stream::empty()),
                })
            })
        }
    }
    let upstream = Arc::new(Racing {
        hits: AtomicUsize::new(0),
        release: tokio::sync::Notify::new(),
    });
    let output = Capture::default();
    let (dependencies, _, _) = deps(Arc::<Racing>::clone(&upstream), &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let path = play("overlap", "seg.ts", &upstream_url("dead", "seg.ts"));
    let first_path = path.clone();
    let port = server.port();
    let first = tokio::spawn(async move { Conn::open(port).await.get(&first_path).await });
    assert!(eventually(|| upstream.hits.load(Ordering::SeqCst) == 1, WITHIN).await);
    let mut other = Conn::open(port).await;
    assert_eq!(other.get(&path).await.status, 503);
    assert_eq!(other.get(&path).await.status, 503);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);
    upstream.release.notify_one();
    assert_eq!(first.await.expect("first").status, 200);
    assert_eq!(other.get(&path).await.status, 200);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 3);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_real_transport_marks_redirected_responses_untrusted() {
    let stub = stub_origin().await;
    let upstream = ReqwestUpstream::with_overrides(tls(), &[("us1.vrcresolver.com", stub.addr)])
        .expect("upstream");
    for (path, direct) in [("full/video.mp4", true), ("hop/x.ts", false)] {
        let response = upstream
            .send(UpstreamRequest {
                method: Method::GET,
                url: Url::parse(&format!(
                    "http://us1.vrcresolver.com:{}/api/proxy/{path}",
                    stub.addr.port()
                ))
                .expect("URL"),
                headers: HeaderMap::new(),
            })
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.direct, direct);
    }
}
