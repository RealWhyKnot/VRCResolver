use super::*;

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
