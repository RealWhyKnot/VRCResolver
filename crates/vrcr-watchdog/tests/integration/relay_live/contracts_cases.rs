use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thirty_back_to_back_plays_share_one_connection() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    for i in 0..30 {
        let r = c
            .get(&play(
                &format!("p{i}"),
                "video.mp4",
                &upstream_url("full-20000", "video.mp4"),
            ))
            .await;
        assert_eq!(
            (r.status, r.body.len(), r.complete),
            (200, 20000, true),
            "play {i}"
        );
    }
    assert_eq!(h.script.seen_urls().len(), 30);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perf_window_counts_relay_traffic_only_when_enabled() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    let r = c
        .get(&play(
            "perf0",
            "manifest.m3u8",
            &upstream_url("hls", "index.m3u8"),
        ))
        .await;
    assert_eq!(r.status, 200);
    assert!(eventually(|| h.server.in_flight() == 0, WITHIN).await);
    assert!(!h.stats.perf.take().is_active());

    h.stats.perf.enable();
    let r = c
        .get(&play(
            "perf1",
            "manifest.m3u8",
            &upstream_url("hls", "index.m3u8"),
        ))
        .await;
    let text = String::from_utf8(r.body).unwrap_or_else(|e| panic!("{e}"));
    let segments: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    for seg in &segments {
        let r = c.get(&format!("/play/perf1/{seg}")).await;
        assert_eq!(r.status, 200, "{seg}");
    }
    c.send("HEAD", &format!("/play/perf1/{}", segments[0]), &[])
        .await;
    assert_eq!(c.reply(true).await.status, 200);
    assert!(eventually(|| h.server.in_flight() == 0, WITHIN).await);
    let mut snap = h.stats.perf.take();
    assert_eq!(snap.relay_requests, 4);
    assert_eq!(snap.relay_errors, 0);
    assert!(snap.relay_bytes >= 3000, "{}", snap.relay_bytes);
    assert_eq!(snap.segment_total.spread().map(|s| s.count), Some(3));
    assert_eq!(snap.segment_header.spread().map(|s| s.count), Some(3));
    assert_eq!(snap.manifest_total.spread().map(|s| s.count), Some(1));
    assert!(!h.stats.perf.take().is_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_without_a_certificate_refuses_to_start() {
    let out = Capture::default();
    let (deps, _, _) = deps(Arc::new(Script::default()), &out);
    let mut cfg = RelayConfig::http(0);
    cfg.scheme = "https".to_owned();
    assert!(RelayServer::start(cfg, deps).await.is_err());
    assert!(
        out.text().contains("no certificate loaded"),
        "{}",
        out.text()
    );
}
