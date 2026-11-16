use super::*;

const LIVE: &str = "https://us1.vrcresolver.com/api/proxy/live/index.m3u8";

fn through_server(origin: &str) -> String {
    format!(
        "https://us1.vrcresolver.com/api/proxy/seg.ts?url={}",
        vrcr_shared::b64::encode(origin)
    )
}

async fn segment(h: &Harness, origin: &str) -> Reply {
    let mut c = Conn::open(h.server.port()).await;
    c.get(&play("live1", "seg.ts", &through_server(origin)))
        .await
}

#[tokio::test]
async fn a_live_video_the_server_keeps_failing_switches_to_direct_and_back() {
    let h = harness(None).await;
    let mut c = Conn::open(h.server.port()).await;
    assert_eq!(c.get(&play("live1", "index.m3u8", LIVE)).await.status, 200);

    for n in 1..=3 {
        let origin = format!("https://video-edge.hls.ttvnw.net/v1/{n}/hls/seg.ts");
        assert_eq!(segment(&h, &origin).await.status, 502);
        assert_eq!(h.script.seen_urls().last(), Some(&through_server(&origin)));
    }
    assert!(h.out.logs().contains("switching it to direct playback"));

    let working = "https://video-edge.hls.ttvnw.net/v1/4/hls/seg.ts";
    let reply = segment(&h, working).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.len(), 1000);
    let direct = h.script.snapshot().pop().expect("the relay sent a request");
    assert_eq!(direct.url, working);
    assert!(direct.headers.get("x-request-id").is_none());

    let broken = "https://video-edge.hls.ttvnw.net/v1/5/fail/seg.ts";
    assert_eq!(segment(&h, broken).await.status, 502);
    let urls = h.script.seen_urls();
    assert_eq!(
        urls[urls.len() - 2..],
        [broken.to_owned(), through_server(broken)]
    );
    assert!(h.out.logs().contains("going back through the server"));

    let after = "https://video-edge.hls.ttvnw.net/v1/6/hls/seg.ts";
    assert_eq!(segment(&h, after).await.status, 502);
    assert_eq!(h.script.seen_urls().last(), Some(&through_server(after)));
}

#[tokio::test]
async fn a_video_without_a_live_playlist_never_leaves_the_server() {
    let h = harness(None).await;
    for n in 1..=5 {
        let origin = format!("https://video-edge.hls.ttvnw.net/v1/{n}/hls/seg.ts");
        assert_eq!(segment(&h, &origin).await.status, 502);
    }
    assert!(
        h.script
            .seen_urls()
            .iter()
            .all(|url| url.starts_with("https://us1.vrcresolver.com/"))
    );
    assert!(!h.out.logs().contains("direct playback"));
}
