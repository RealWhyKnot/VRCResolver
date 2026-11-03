use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalls_and_failures_hit_the_timeouts() {
    let h = harness(Some(Duration::from_millis(500))).await;
    let port = h.server.port();

    let mut c = Conn::open(port).await;
    let r = c
        .get(&play(
            "sh",
            "seg.ts",
            &upstream_url("stall-headers", "seg.ts"),
        ))
        .await;
    assert_eq!(r.status, 504);

    let mut c = Conn::open(port).await;
    let r = c
        .get(&play("sb", "seg.ts", &upstream_url("stall-body", "seg.ts")))
        .await;
    assert_eq!(r.status, 200);
    assert!(!r.complete);

    let mut c = Conn::open(port).await;
    let r = c
        .get(&play(
            "ms",
            "index.m3u8",
            &upstream_url("manifest-stall", "index.m3u8"),
        ))
        .await;
    assert_eq!(r.status, 200);
    assert!(!r.complete);

    let mut c = Conn::open(port).await;
    let r = c
        .get(&play("f", "seg.ts", &upstream_url("fail", "seg.ts")))
        .await;
    assert_eq!(r.status, 502);
    for code in [404u16, 500] {
        let mode = format!("status-{code}");
        let r = c
            .get(&play("s", "seg.ts", &upstream_url(&mode, "seg.ts")))
            .await;
        assert_eq!(
            (r.status, r.body.as_slice(), r.complete),
            (code, &b"nope"[..], true)
        );
    }

    assert!(eventually(|| h.script.open() == 0 && h.server.in_flight() == 0, WITHIN).await);
    let text = h.out.text();
    assert!(
        text.contains("upstream timed out for us1.vrcresolver.com"),
        "{text}"
    );
    assert!(text.contains("stream idle timeout for"), "{text}");
    assert!(
        text.matches("upstream timed out for us1.vrcresolver.com")
            .count()
            >= 2,
        "{text}"
    );
    assert!(
        text.contains("request failed for us1.vrcresolver.com: connect_error: refused"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_client_is_aborted_while_the_next_request_is_served() {
    let h = harness(Some(Duration::from_millis(500))).await;
    let port = h.server.port();
    let mut stalled = Conn::open(port).await;
    stalled
        .send(
            "GET",
            &play("st", "seg.ts", &upstream_url("endless", "seg.ts")),
            &[],
        )
        .await;
    assert!(eventually(|| h.script.open() == 1, WITHIN).await);

    let mut next = Conn::open(port).await;
    let r = next
        .get(&play("nx", "seg.ts", &upstream_url("full-1000", "seg.ts")))
        .await;
    assert_eq!((r.status, r.body.len(), r.complete), (200, 1000, true));

    assert!(eventually(|| h.script.open() == 0, Duration::from_secs(10)).await);
    assert!(
        h.out.text().contains("client write stalled for"),
        "{}",
        h.out.text()
    );
    let r = tokio::time::timeout(Duration::from_secs(10), stalled.read_reply(false))
        .await
        .unwrap_or_else(|_| panic!("stalled client never saw the reset"));
    assert!(!r.complete);
    assert!(eventually(|| h.server.in_flight() == 0, WITHIN).await);
}
