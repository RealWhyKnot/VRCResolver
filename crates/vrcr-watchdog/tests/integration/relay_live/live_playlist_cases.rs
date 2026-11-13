use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_playlist_client_abort_releases_a_pending_manifest() {
    let harness = harness(None).await;
    let mut connection = Conn::open(harness.server.port()).await;
    connection
        .send(
            "GET",
            &play(
                "abort-live",
                "index.m3u8",
                &upstream_url("manifest-stall", "index.m3u8"),
            ),
            &[],
        )
        .await;
    assert!(eventually(|| harness.script.open() == 1, WITHIN).await);
    drop(connection);
    assert!(
        eventually(
            || harness.script.open() == 0 && harness.server.in_flight() == 0,
            WITHIN
        )
        .await
    );
}

struct LivePlaylist {
    body: String,
    last_url: String,
    hits: AtomicUsize,
    open: Arc<AtomicUsize>,
    advance: AtomicUsize,
    segment_urls: Mutex<Vec<String>>,
}

impl LivePlaylist {
    fn new() -> Self {
        let (body, last_url) = Self::contents(0);
        Self {
            body,
            last_url,
            hits: AtomicUsize::new(0),
            open: Arc::default(),
            advance: AtomicUsize::new(0),
            segment_urls: Mutex::default(),
        }
    }

    fn contents(start: usize) -> (String, String) {
        let mut body = format!("#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:{start}\n");
        let mut last_url = String::new();
        for segment in start..start + 2880 {
            let base =
                format!("https://us1.vrcresolver.com/api/proxy/live/seg_{segment:06}.ts?token=");
            last_url = format!("{base}{}", "x".repeat(1900 - base.len()));
            writeln!(body, "#EXTINF:5,\n{last_url}").expect("playlist");
        }
        (body, last_url)
    }
}

impl Upstream for LivePlaylist {
    fn send(&self, request: UpstreamRequest) -> Sending<'_> {
        Box::pin(async move {
            assert_eq!(request.headers["accept-encoding"], "identity");
            self.hits.fetch_add(1, Ordering::SeqCst);
            if request.url.path().ends_with(".m3u8") {
                let advance = self.advance.load(Ordering::SeqCst);
                let body = if advance == 0 {
                    self.body.clone()
                } else {
                    Self::contents(advance).0
                };
                Ok(respond(
                    &self.open,
                    200,
                    &[("content-type", "application/vnd.apple.mpegurl".to_owned())],
                    body.as_bytes()
                        .chunks(4096)
                        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                        .collect(),
                    Tail::End,
                ))
            } else {
                self.segment_urls
                    .lock()
                    .expect("segments")
                    .push(request.url.to_string());
                Ok(respond(
                    &self.open,
                    200,
                    &[("content-length", "1000".to_owned())],
                    filled(1000, b's'),
                    Tail::End,
                ))
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_live_playlist_is_trimmed_before_localizing_and_segments_still_relay() {
    let origin = Arc::new(LivePlaylist::new());
    let output = Capture::default();
    let (dependencies, stats, _) = deps(Arc::<LivePlaylist>::clone(&origin), &output);
    dependencies.log.set_dev_console(true);
    stats.perf.enable();
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let mut connection = Conn::open(server.port()).await;
    let reply = connection
        .get(&play(
            "live",
            "index.m3u8",
            "https://us1.vrcresolver.com/api/proxy/live/index.m3u8",
        ))
        .await;
    assert_eq!(reply.status, 200);
    assert!(reply.complete);
    assert!(reply.body.len() < 64_000, "{} bytes", reply.body.len());
    let body = String::from_utf8(reply.body).expect("playlist text");
    assert_eq!(body.matches("#EXTINF:").count(), 12);
    assert!(body.contains("#EXT-X-MEDIA-SEQUENCE:2868"));
    let last = body
        .lines()
        .rfind(|line| !line.starts_with('#') && !line.is_empty())
        .expect("last segment");
    assert!(last.contains("seg_002879.ts"));
    let segment = connection.get(&format!("/play/live/{last}")).await;
    assert_eq!(segment.status, 200);
    assert!(segment.complete);
    assert_eq!(segment.body, vec![b's'; 1000]);
    assert_eq!(
        origin.segment_urls.lock().expect("segments").as_slice(),
        std::slice::from_ref(&origin.last_url)
    );
    assert!(
        eventually(
            || server.in_flight() == 0 && origin.open.load(Ordering::SeqCst) == 0,
            WITHIN
        )
        .await
    );
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
    let diagnostics = output.logs();
    assert!(
        !diagnostics.contains("large-media-playlist"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains(&format!(
            "playlist-bytes={} segments=2880 trimmed=true kept=12",
            origin.body.len()
        )),
        "{diagnostics}"
    );
    let perf = stats.perf.take();
    assert_eq!(perf.relay_requests, 2);
    assert_eq!(perf.relay_errors, 0);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_fifty_reloads_log_one_quiet_trim_and_old_segments_survive_window_advance() {
    let origin = Arc::new(LivePlaylist::new());
    let output = Capture::default();
    let (dependencies, _, _) = deps(Arc::<LivePlaylist>::clone(&origin), &output);
    dependencies.log.set_dev_console(true);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let mut connection = Conn::open(server.port()).await;
    let path = play(
        "reloads",
        "index.m3u8",
        "https://us1.vrcresolver.com/api/proxy/live/index.m3u8",
    );
    let mut oldest = String::new();
    for reload in 0..50 {
        let reply = connection.get(&path).await;
        assert!(reply.complete);
        let body = String::from_utf8(reply.body).expect("text");
        assert_eq!(body.matches("#EXTINF:").count(), 12);
        if reload == 0 {
            oldest = body
                .lines()
                .find(|line| !line.starts_with('#') && !line.is_empty())
                .expect("oldest")
                .to_owned();
        }
    }
    assert!(eventually(|| server.in_flight() == 0, WITHIN).await);
    let log = output.logs();
    assert_eq!(log.matches("playlist trimmed").count(), 1);
    assert!(!log.contains("[warn] playlist trimmed"));
    assert!(!log.contains("large-media-playlist"));
    origin.advance.store(30, Ordering::SeqCst);
    let reply = connection.get(&path).await;
    assert!(reply.complete);
    let body = String::from_utf8(reply.body).expect("text");
    assert!(body.contains("#EXT-X-MEDIA-SEQUENCE:2898"));
    assert!(!body.contains("seg_002868.ts"));
    let reply = connection.get(&format!("/play/reloads/{oldest}")).await;
    assert_eq!(reply.status, 200);
    assert!(reply.complete);
    assert_eq!(reply.body, vec![b's'; 1000]);
    assert!(origin.segment_urls.lock().expect("segments")[0].contains("seg_002868.ts"));
    server.stop().await;
}
