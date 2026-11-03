use super::*;

#[tokio::test]
async fn idle_connections_do_not_block_new_requests() {
    let h = harness(None).await;
    let mut connections = Vec::new();
    for _ in 0..128 {
        connections.push(Conn::open(h.server.port()).await);
    }
    let mut client = Conn::open(h.server.port()).await;
    client.send("GET", "/missing", &[]).await;
    let reply = client.read_reply(false).await;
    assert_eq!(reply.status, 404);
    assert_eq!(h.script.snapshot().len(), 0);
    h.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_rejects_before_touching_upstream() {
    let h = harness(None).await;
    let port = h.server.port();
    let good = play("gate", "seg.ts", &upstream_url("full-10", "seg.ts"));
    let mut c = Conn::open(port).await;

    c.send("GET", &good, &[("Origin", "https://evil.example")])
        .await;
    assert_eq!(c.reply(false).await.status, 403);
    assert_eq!(c.get("/not-play/gate/seg.ts").await.status, 404);
    c.send("POST", &good, &[("Content-Length", "0")]).await;
    let r = c.reply(false).await;
    assert_eq!(r.status, 405);
    assert_eq!(r.header("allow"), Some("GET, HEAD"));
    assert_eq!(c.get("/play/gate/seg.ts").await.status, 400);
    let evil = play("gate", "seg.ts", "https://example.com/api/proxy/seg.ts");
    assert_eq!(c.get(&evil).await.status, 403);
    let file = play("gate", "seg.ts", "file:///C:/Windows/win.ini");
    assert_eq!(c.get(&file).await.status, 403);
    c.send_raw(&format!(
        "GET {good} HTTP/1.1\r\nHost: us1.vrcresolver.com:{port}\r\n\r\n"
    ))
    .await;
    assert_eq!(c.reply(false).await.status, 403);
    c.send_raw(&format!(
        "GET {good} HTTP/1.1\r\nHost: localhost.youtube.com:{}\r\n\r\n",
        port.wrapping_add(1)
    ))
    .await;
    assert_eq!(c.reply(false).await.status, 403);

    assert_eq!(h.script.seen_urls(), <[std::string::String; 0]>::default());
    assert_eq!(
        h.activity.wedged_port(&format!(
            "https://localhost.youtube.com:{port}/play/gate/seg.ts"
        )),
        None
    );
    assert_eq!(h.stats.activity().relay_requests, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_body_streams_with_cleaned_headers() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    c.send(
        "GET",
        &play("full", "seg.ts", &upstream_url("full-100000", "seg.ts")),
        &[
            ("Cookie", "session=secret"),
            ("Authorization", "Bearer secret"),
            ("Accept-Encoding", "gzip, br"),
            ("User-Agent", "AVProMobileVideo"),
        ],
    )
    .await;
    let r = c.reply(false).await;
    assert_eq!(r.status, 200);
    assert!(r.complete);
    assert_eq!(r.header("content-length"), Some("100000"));
    assert_eq!(r.header("content-type"), Some("video/mp2t"));
    assert_eq!(r.header("x-lazy-hls"), Some("HIT"));
    assert_eq!(r.header("cf-ray"), None);
    assert_eq!(r.body.len(), 100_000);
    assert!(r.body.iter().all(|b| *b == b'v'));

    let seen = h.script.snapshot();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, Method::GET);
    assert_eq!(seen[0].url, upstream_url("full-100000", "seg.ts"));
    assert!(seen[0].headers.get("cookie").is_none());
    assert!(seen[0].headers.get("authorization").is_none());
    assert!(seen[0].headers.get("host").is_none());
    assert_eq!(seen[0].headers["accept-encoding"], "identity");
    assert_eq!(seen[0].headers["user-agent"], "AVProMobileVideo");

    assert!(
        eventually(
            || h.stats.activity().upstream_relay_bytes == 100_000,
            WITHIN
        )
        .await
    );
    let a = h.stats.activity();
    assert_eq!((a.relay_requests, a.upstream_relay_requests), (1, 1));
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_and_head_keep_the_connection_usable() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    let path = play("rng", "video.mp4", &upstream_url("full-5000", "video.mp4"));

    c.send("GET", &path, &[("Range", "bytes=0-99")]).await;
    let r = c.reply(false).await;
    assert_eq!(r.status, 206);
    assert_eq!(r.header("content-range"), Some("bytes 0-99/5000"));
    assert_eq!(r.body.len(), 100);
    assert!(r.complete);

    c.send("HEAD", &path, &[]).await;
    let r = c.reply(true).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-length"), Some("5000"));

    let r = c.get(&path).await;
    assert_eq!((r.status, r.body.len(), r.complete), (200, 5000, true));

    let seen = h.script.snapshot();
    assert_eq!(seen[0].headers["range"], "bytes=0-99");
    assert_eq!(seen[1].method, Method::HEAD);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_upstream_bodies_end_the_client_response_quickly() {
    let h = harness(None).await;
    let port = h.server.port();
    for mode in ["short", "throw"] {
        let mut c = Conn::open(port).await;
        let started = Instant::now();
        let r = c
            .get(&play(mode, "seg.ts", &upstream_url(mode, "seg.ts")))
            .await;
        assert_eq!(r.status, 200, "{mode}");
        assert!(!r.complete, "{mode}");
        assert!(r.body.len() <= 500, "{mode}");
        assert!(started.elapsed() < WITHIN, "{mode}");
    }
    let mut c = Conn::open(port).await;
    let r = c
        .get(&play("ok", "seg.ts", &upstream_url("full-1000", "seg.ts")))
        .await;
    assert!(r.complete);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
    let text = h.out.text();
    assert!(
        text.contains("upstream ended early (500/1000 bytes)"),
        "{text}"
    );
    assert!(text.contains("upstream_body_error: boom"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_leaving_releases_the_upstream() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    c.send(
        "GET",
        &play("lv", "seg.ts", &upstream_url("endless", "seg.ts")),
        &[],
    )
    .await;
    assert!(c.fill().await);
    assert!(eventually(|| h.script.open() == 1, WITHIN).await);
    drop(c);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_mid_stream_drops_everything() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    c.send(
        "GET",
        &play("sd", "seg.ts", &upstream_url("endless", "seg.ts")),
        &[],
    )
    .await;
    assert!(c.fill().await);
    tokio::time::timeout(WITHIN, h.server.stop())
        .await
        .unwrap_or_else(|_| panic!("stop hung"));
    let r = tokio::time::timeout(Duration::from_secs(10), c.read_reply(false))
        .await
        .unwrap_or_else(|_| panic!("client never saw the close"));
    assert!(!r.complete);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, h.server.port()))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reqwest_upstream_refuses_internal_targets() {
    let up = ReqwestUpstream::new(tls()).unwrap_or_else(|e| panic!("{e}"));
    for target in [
        "http://127.0.0.1/api/proxy/x.ts",
        "http://localhost:9/api/proxy/x.ts?sig=secret",
        "http://169.254.169.254/latest/meta-data",
        "http://10.0.0.5/api/proxy/x.ts",
        "http://[::1]/api/proxy/x.ts",
    ] {
        let req = UpstreamRequest {
            method: Method::GET,
            url: Url::parse(target).unwrap_or_else(|e| panic!("{e}")),
            headers: HeaderMap::new(),
        };
        match up.send(req).await {
            Ok(_) => panic!("{target} was not refused"),
            Err(e) => {
                assert_eq!(e.kind, "blocked_address", "{target}: {e}");
                assert!(
                    e.message.contains("blocked internal address"),
                    "{target}: {e}"
                );
                assert!(!e.message.contains("/api/proxy"), "{target}: {e}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_streams_through_the_real_http_client() {
    let stub = stub_origin().await;
    let up = ReqwestUpstream::with_overrides(tls(), &[("us1.vrcresolver.com", stub.addr)])
        .unwrap_or_else(|e| panic!("{e}"));
    let out = Capture::default();
    let (deps, stats, _) = deps(Arc::new(up), &out);
    let server = RelayServer::start(RelayConfig::http(0), deps)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let origin = format!("http://us1.vrcresolver.com:{}", stub.addr.port());
    let mut c = Conn::open(server.port()).await;

    c.send(
        "GET",
        &play(
            "rq",
            "video.mp4",
            &format!("{origin}/api/proxy/full/video.mp4"),
        ),
        &[("Cookie", "session=secret")],
    )
    .await;
    let r = c.reply(false).await;
    assert_eq!((r.status, r.body.len(), r.complete), (200, 50000, true));
    assert_eq!(r.header("content-type"), Some("video/mp4"));

    let mut c = Conn::open(server.port()).await;
    let r = c
        .get(&play("rq", "x.ts", &format!("{origin}/api/proxy/hop/x.ts")))
        .await;
    assert_eq!((r.status, r.body.len()), (200, 50000));

    let mut c = Conn::open(server.port()).await;
    let r = c
        .get(&play(
            "rq",
            "x.ts",
            &format!("{origin}/api/proxy/redirect/x.ts"),
        ))
        .await;
    assert_eq!(r.status, 502);
    let text = out.text();
    assert!(text.contains("blocked_address"), "{text}");
    assert!(
        text.contains("blocked internal address (127.0.0.1)"),
        "{text}"
    );

    let heads = stub.heads.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(heads.len(), 4, "{heads:?}");
    let first = heads[0].to_ascii_lowercase();
    assert!(first.contains("accept-encoding: identity"), "{first}");
    assert!(!first.contains("cookie"), "{first}");
    assert!(
        first.contains(&format!("host: us1.vrcresolver.com:{}", stub.addr.port())),
        "{first}"
    );
    assert!(eventually(|| stats.activity().upstream_relay_bytes == 100_000, WITHIN).await);
    server.stop().await;
}
