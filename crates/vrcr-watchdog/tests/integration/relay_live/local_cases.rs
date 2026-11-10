use vrcr_watchdog::local;

use super::*;

const BODY: &str = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nhttps://media.example/v.m3u8\n";

struct LocalHarness {
    server: RelayServer,
    script: Arc<Script>,
    activity: Arc<RelayActivity>,
    path: String,
}

async fn local_harness() -> LocalHarness {
    let script = Arc::new(Script::default());
    let out = Capture::default();
    let (deps, _, activity) = deps(Arc::<Script>::clone(&script), &out);
    let token = deps.manifests.insert(
        BODY.to_owned(),
        "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
    );
    let server = RelayServer::start(RelayConfig::http(0), deps)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    LocalHarness {
        server,
        script,
        activity,
        path: format!("{}{token}/{}", local::PATH_PREFIX, local::MANIFEST_NAME),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stored_manifest_is_served_without_going_upstream() {
    let h = local_harness().await;
    let mut c = Conn::open(h.server.port()).await;
    for _ in 0..2 {
        let r = c.get(&h.path).await;
        assert_eq!(r.status, 200);
        assert_eq!(r.header("content-type"), Some(local::MANIFEST_TYPE));
        assert_eq!(r.header("cache-control"), Some("no-store"));
        assert_eq!(String::from_utf8_lossy(&r.body), BODY);
    }
    c.send("HEAD", &h.path, &[]).await;
    let r = c.reply(true).await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("content-length"),
        Some(BODY.len().to_string().as_str())
    );
    assert_eq!(r.body.len(), 0);
    assert_eq!(h.script.seen_urls(), Vec::<String>::new());
    assert!(eventually(|| h.server.in_flight() == 0, WITHIN).await);
    let opened = format!("http://localhost.youtube.com:{}{}", h.server.port(), h.path);
    assert_eq!(h.activity.wedged_port(&opened), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_and_cross_site_manifest_requests_are_turned_away() {
    let h = local_harness().await;
    let mut c = Conn::open(h.server.port()).await;
    let unknown = format!("{}nope/{}", local::PATH_PREFIX, local::MANIFEST_NAME);
    assert_eq!(c.get(&unknown).await.status, 404);
    c.send("GET", &h.path, &[("Origin", "https://evil.example")])
        .await;
    assert_eq!(c.reply(false).await.status, 403);
    c.send("POST", &h.path, &[("Content-Length", "0")]).await;
    assert_eq!(c.reply(false).await.status, 405);
    assert_eq!(h.script.seen_urls(), Vec::<String>::new());
}
