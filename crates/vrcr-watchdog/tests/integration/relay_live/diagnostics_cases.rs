use super::*;

#[derive(Default)]
struct Missing {
    ids: Mutex<Vec<String>>,
    open: Arc<AtomicUsize>,
}

impl Upstream for Missing {
    fn send(&self, request: UpstreamRequest) -> Sending<'_> {
        self.ids.lock().expect("ids").push(
            request
                .headers
                .get("x-request-id")
                .expect("request id header")
                .to_str()
                .expect("ASCII id")
                .to_owned(),
        );
        Box::pin(async move {
            Ok(respond(
                &self.open,
                404,
                &[("content-length", "1".to_owned())],
                vec![Ok(Bytes::from_static(b"x"))],
                Tail::End,
            ))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_five_hundred_missing_segments_have_two_counted_lines_and_one_summary() {
    let upstream = Arc::new(Missing::default());
    let output = Capture::default();
    let (dependencies, _, _) = deps(Arc::<Missing>::clone(&upstream), &output);
    let mut config = RelayConfig::http(0);
    config.verbose = true;
    let server = RelayServer::start(config, dependencies)
        .await
        .expect("relay");
    let target = upstream_url("missing", "seg_000001.ts");
    let path = play("dead", "seg_000001.ts", &format!("{target}?secret=value"));
    let mut connection = Conn::open(server.port()).await;
    for _ in 0..500 {
        let reply = connection.get(&path).await;
        assert_eq!(reply.status, 404);
        assert!(reply.complete);
        assert_eq!(reply.body, b"x");
    }
    assert!(eventually(|| server.in_flight() == 0, WITHIN).await);
    server.stop().await;
    let ids = upstream.ids.lock().expect("ids");
    assert_eq!(ids.len(), 500);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        500
    );
    assert!(ids.iter().all(|id| {
        id.len() <= 80
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }));
    let text = output.logs();
    assert!(text.contains(&format!("req={}", ids[0])));
    assert!(text.contains(&format!("req={}", ids[499])));
    drop(ids);
    assert_eq!(text.matches("request failed").count(), 2, "{text}");
    assert_eq!(text.matches("session playback=").count(), 1, "{text}");
    assert!(text.contains("kind=segment host=us1.vrcresolver.com status=404"));
    assert!(text.contains(
        "bytes=1 retry=true sent=404 reason=upstream_status hitches=segment-http-404 body=complete count=500"
    ));
    assert!(text.contains("manifests=0 segments=500 other=0 bytes=500"));
    assert!(
        text.contains("hitches=segment-http-404:500 retries=499 statuses=404:500 ended=shutdown")
    );
    assert!(!text.contains("secret"));
    assert!(!text.contains("/api/"));
    assert!(text.is_ascii());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_refusals_and_http_parse_failures_have_ids_and_dispositions() {
    let harness = harness(None).await;
    let mut connection = Conn::open(harness.server.port()).await;
    connection
        .send("POST", "/play/refused/index.m3u8", &[])
        .await;
    assert_eq!(connection.reply(false).await.status, 405);
    connection
        .send(
            "GET",
            "/play/refused/index.m3u8",
            &[("Origin", "https://private.example/path?secret=x")],
        )
        .await;
    assert_eq!(connection.reply(false).await.status, 403);
    let mut malformed = Conn::open(harness.server.port()).await;
    malformed
        .stream
        .write_all(b"GET /private?secret=x HTTP/1.1\r\nbad header\r\n\r\n")
        .await
        .expect("write");
    assert_eq!(malformed.reply(false).await.status, 400);
    assert!(eventually(|| harness.out.logs().contains("http_parse_failed"), WITHIN).await);
    let text = harness.out.logs();
    assert!(text.contains("request failed req=relay-"));
    assert!(text.contains("bytes=0 retry=false sent=405 reason=method_refused"));
    assert!(text.contains("bytes=0 retry=false sent=403 reason=origin_refused"));
    assert!(text.contains("request failed req=http-"));
    assert!(text.contains("kind=connection host=unknown status=0"));
    assert!(text.contains("sent=0 reason=http_parse_failed hitches=none body=unknown count=1"));
    assert!(!text.contains("private"));
    assert!(!text.contains("secret"));
    harness.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_verbose_completion_is_one_file_line_without_a_url() {
    let directory = tempfile::TempDir::new().expect("logs");
    let log_path = directory.path().join("relay.log");
    let script = Arc::new(Script::default());
    let output = Capture::default();
    let (mut dependencies, _, _) = deps(Arc::<Script>::clone(&script), &output);
    dependencies.log = Log::to_file(&log_path);
    let log = dependencies.log.clone();
    let mut config = RelayConfig::http(0);
    config.verbose = true;
    let server = RelayServer::start(config, dependencies)
        .await
        .expect("relay");
    let mut connection = Conn::open(server.port()).await;
    let target = upstream_url("full-1000", "seg.ts");
    let reply = connection.get(&play("verbose", "seg.ts", &target)).await;
    assert_eq!(reply.status, 200);
    assert!(eventually(|| server.in_flight() == 0, WITHIN).await);
    server.stop().await;
    log.close();
    let text = std::fs::read_to_string(log_path).expect("log");
    assert_eq!(text.matches("[relay][debug]").count(), 1, "{text}");
    let id = script.snapshot()[0].headers["x-request-id"]
        .to_str()
        .expect("id")
        .to_owned();
    assert!(
        text.contains(&format!(
            "req={id} kind=segment host=us1.vrcresolver.com status=200"
        )),
        "{text}"
    );
    assert!(text.contains("sent=200"));
    assert!(!text.contains("/api/"));
    assert!(!text.contains("https://"));
    assert!(!output.logs().contains("[debug]"));
}
