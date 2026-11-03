use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hls_manifest_then_every_segment() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    let r = c
        .get(&play(
            "hls1",
            "manifest.m3u8",
            &upstream_url("hls", "index.m3u8"),
        ))
        .await;
    assert_eq!(r.status, 200);
    assert!(r.complete);
    assert_eq!(
        r.header("content-type"),
        Some("application/vnd.apple.mpegurl")
    );
    assert_eq!(r.header("content-length"), None);
    let text = String::from_utf8(r.body).unwrap_or_else(|e| panic!("{e}"));
    assert!(!text.contains("https://"), "{text}");
    let segments: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    assert_eq!(segments.len(), 3, "{text}");
    for seg in &segments {
        let r = c.get(&format!("/play/hls1/{seg}")).await;
        assert_eq!(
            (r.status, r.body.len(), r.complete),
            (200, 1000, true),
            "{seg}"
        );
    }
    let urls = h.script.seen_urls();
    assert_eq!(
        urls,
        vec![
            upstream_url("hls", "index.m3u8"),
            upstream_url("hls", "seg_000001.ts?playback_id=t"),
            upstream_url("hls", "seg_000002.ts?playback_id=t"),
            upstream_url("hls", "seg_000003.ts"),
        ]
    );
    assert!(h.server.remembered_bases() >= 1);
    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
}
