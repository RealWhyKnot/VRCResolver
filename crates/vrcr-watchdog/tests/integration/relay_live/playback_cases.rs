use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_proxy_sentences_repeat_only_after_recovery() {
    for (reason, sentence) in [
        ("unknown_stream", "The server no longer has this stream."),
        (
            "stream_recreate_required",
            "The server asked to restart the stream.",
        ),
        ("rate_limited", "The server is rate-limiting this stream."),
        (
            "upstream_failed",
            "The video site returned an error to the server.",
        ),
        (
            "temporarily_unavailable",
            "The server is busy or restarting.",
        ),
        (
            "upstream_timeout",
            "The video site took too long to answer the server.",
        ),
        (
            "access_denied",
            "The video site refused the server's request.",
        ),
        ("request_failed", "A request to the server failed."),
        ("invalid_range", ""),
        ("new_reason!/.?", "playback error: new_reason"),
    ] {
        let upstream = origin(403, None, Some(reason), b"");
        let output = Capture::default();
        let (dependencies, _, _) = deps(Arc::<FailingOrigin>::clone(&upstream), &output);
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let mut client = Conn::open(server.port()).await;
        let path = play("notice", "seg.ts", &upstream_url("notice", "seg.ts"));
        for _ in 0..3 {
            assert_eq!(client.get(&path).await.status, 403);
        }
        let notices = || {
            output
                .text()
                .lines()
                .filter(|line| line.contains("[relay][warn]"))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        if sentence.is_empty() {
            assert_eq!(notices(), Vec::<String>::new());
        } else {
            assert_eq!(notices().len(), 1, "{reason}");
            assert!(notices()[0].contains(&format!("{:<28} -- {sentence}", "us1.vrcresolver.com")));
            assert!(!notices()[0].contains("/api/"));
        }
        upstream.status.store(200, Ordering::SeqCst);
        assert_eq!(client.get(&path).await.status, 200);
        assert!(eventually(|| server.in_flight() == 0, WITHIN).await);
        upstream.status.store(403, Ordering::SeqCst);
        assert_eq!(client.get(&path).await.status, 403);
        if !sentence.is_empty() {
            assert_eq!(notices().len(), 2, "{reason}");
        }
        let other = play("other", "seg.ts", &upstream_url("notice", "seg.ts"));
        assert_eq!(client.get(&other).await.status, 403);
        if !sentence.is_empty() {
            assert_eq!(notices().len(), 3, "{reason}");
        }
        assert!(!output.text().contains("[hitch]"));
        server.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_proxy_notice_is_not_repeated_for_cached_failures() {
    let upstream = origin(503, Some("30"), Some("temporarily_unavailable"), b"");
    let output = Capture::default();
    let (dependencies, _) = timed_deps(&upstream, &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let path = play("notice", "seg.ts", &upstream_url("dead", "seg.ts"));
    let mut client = Conn::open(server.port()).await;
    for _ in 0..5 {
        assert_eq!(client.get(&path).await.status, 503);
    }
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        output
            .text()
            .matches("The server is busy or restarting.")
            .count(),
        1
    );
    assert!(output.text().contains("source.example"));
    assert!(!output.text().contains("[hitch]"));
    server.stop().await;
}
