use super::*;
use std::sync::atomic::AtomicBool;
use vrcr_watchdog::relay::manifest;

struct PacedPlaylist {
    bytes: Bytes,
    interval: Duration,
    chunk_size: usize,
    finished: Arc<AtomicBool>,
    fail: bool,
    known_length: bool,
}

impl Upstream for PacedPlaylist {
    fn send(&self, _: UpstreamRequest) -> Sending<'_> {
        let bytes = self.bytes.clone();
        let interval = self.interval;
        let chunk_size = self.chunk_size;
        let finished = Arc::clone(&self.finished);
        let fail = self.fail;
        let length = self.known_length.then_some(self.bytes.len());
        Box::pin(async move {
            let stream =
                futures_util::stream::unfold((bytes, 0, false), move |(bytes, offset, ended)| {
                    let finished = Arc::clone(&finished);
                    async move {
                        if ended {
                            return None;
                        }
                        if offset == bytes.len() {
                            finished.store(true, Ordering::SeqCst);
                            return if fail {
                                Some((
                                    Err(io::Error::other("truncated body")),
                                    (bytes, offset, true),
                                ))
                            } else {
                                None
                            };
                        }
                        if offset > 0 {
                            tokio::time::sleep(interval).await;
                        }
                        let end = (offset + chunk_size).min(bytes.len());
                        Some((Ok(bytes.slice(offset..end)), (bytes, end, false)))
                    }
                });
            let mut headers = HeaderMap::new();
            headers.insert(
                "content-type",
                HeaderValue::from_static("application/vnd.apple.mpegurl"),
            );
            if let Some(length) = length {
                headers.insert("content-length", HeaderValue::from(length));
            }
            Ok(UpstreamResponse {
                direct: false,
                status: StatusCode::OK,
                headers,
                body: Box::pin(stream),
            })
        })
    }
}

fn vod(count: usize) -> String {
    let mut body = "#EXTM3U\n#EXT-X-TARGETDURATION:5\n".to_owned();
    for segment in 0..count {
        writeln!(body, "#EXTINF:5,\nseg_{segment:06}.ts?{}", "x".repeat(1880)).expect("fixture");
    }
    body.push_str("#EXT-X-ENDLIST\n");
    body
}

async fn first_body_chunk(connection: &mut Conn) -> Vec<u8> {
    let head = connection.reply(true).await;
    assert_eq!(head.status, 200);
    let size = connection.line().await.expect("chunk header");
    let size = usize::from_str_radix(&size, 16).expect("size");
    assert!(size > 0);
    assert!(connection.need(size + 2).await);
    let body = connection.buf.drain(..size).collect();
    connection.buf.drain(..2);
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_buffer_budget_streams_large_slow_vod_before_upstream_finishes() {
    let origin = Arc::new(PacedPlaylist {
        bytes: Bytes::from(vod(1800)),
        interval: Duration::from_millis(30),
        chunk_size: 32 * 1024,
        finished: Arc::default(),
        fail: false,
        known_length: false,
    });
    let output = Capture::default();
    let (dependencies, _, _) = deps(Arc::<PacedPlaylist>::clone(&origin), &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let mut connection = Conn::open(server.port()).await;
    let start = Instant::now();
    connection
        .send(
            "GET",
            &play("slow-vod", "index.m3u8", &upstream_url("vod", "index.m3u8")),
            &[],
        )
        .await;
    let first = tokio::time::timeout(
        Duration::from_millis(500),
        first_body_chunk(&mut connection),
    )
    .await
    .expect("first body before full download");
    assert!(first.starts_with(b"#EXTM3U"));
    assert!(start.elapsed() < Duration::from_millis(500));
    assert!(!origin.finished.load(Ordering::SeqCst));
    drop(connection);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_buffer_budget_preserves_vod_partial_body_at_manifest_deadline() {
    let origin = Arc::new(PacedPlaylist {
        bytes: Bytes::from(vod(1800)),
        interval: Duration::from_millis(40),
        chunk_size: 64 * 1024,
        finished: Arc::default(),
        fail: false,
        known_length: false,
    });
    let output = Capture::default();
    let (dependencies, _, _) = deps(Arc::<PacedPlaylist>::clone(&origin), &output);
    let mut config = RelayConfig::http(0);
    config.timeout = Some(Duration::from_millis(300));
    let server = RelayServer::start(config, dependencies)
        .await
        .expect("relay");
    let mut connection = Conn::open(server.port()).await;
    let reply = connection
        .get(&play(
            "deadline",
            "index.m3u8",
            &upstream_url("vod", "index.m3u8"),
        ))
        .await;
    assert_eq!(reply.status, 200);
    assert!(!reply.complete);
    assert!(reply.body.starts_with(b"#EXTM3U"));
    assert!(reply.body.len() > 8192);
    assert!(!origin.finished.load(Ordering::SeqCst));
    assert!(output.logs().contains("reason=upstream_timeout"));
    server.stop().await;
}

#[tokio::test(start_paused = true)]
async fn live_buffer_budget_keeps_small_vod_first_byte_timing() {
    let origin = PacedPlaylist {
        bytes: Bytes::from(vod(2)),
        interval: Duration::from_millis(10),
        chunk_size: 2048,
        finished: Arc::default(),
        fail: false,
        known_length: false,
    };
    let request = || UpstreamRequest {
        method: Method::GET,
        url: Url::parse("https://us1.vrcresolver.com/api/proxy/vod.m3u8").expect("url"),
        headers: HeaderMap::new(),
    };
    let policy = FirstPartyPolicy::default();
    let mut old = Vec::new();
    let started = tokio::time::Instant::now();
    manifest::localize_stream(
        &policy,
        origin.send(request()).await.expect("response").body,
        &mut old,
        manifest::MAX_MANIFEST_BYTES,
    )
    .await
    .expect("old localizer");
    let old_time = started.elapsed();
    let mut new = Vec::new();
    let started = tokio::time::Instant::now();
    manifest::localize_relay_stream(
        &policy,
        origin.send(request()).await.expect("response").body,
        &mut new,
        manifest::MAX_MANIFEST_BYTES,
    )
    .await
    .expect("new localizer");
    assert_eq!(started.elapsed(), old_time);
    assert_eq!(old, new);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_known_small_vod_streams_before_the_next_upstream_chunk() {
    let origin = Arc::new(PacedPlaylist {
        bytes: Bytes::from(vod(40)),
        interval: Duration::from_secs(1),
        chunk_size: 16 * 1024,
        finished: Arc::default(),
        fail: false,
        known_length: true,
    });
    let output = Capture::default();
    let (dependencies, _, _) = deps(Arc::<PacedPlaylist>::clone(&origin), &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let mut connection = Conn::open(server.port()).await;
    connection
        .send(
            "GET",
            &play(
                "small-vod",
                "index.m3u8",
                &upstream_url("vod", "index.m3u8"),
            ),
            &[],
        )
        .await;
    let first = tokio::time::timeout(
        Duration::from_millis(500),
        first_body_chunk(&mut connection),
    )
    .await
    .expect("first chunk");
    assert!(first.starts_with(b"#EXTM3U"));
    assert!(!origin.finished.load(Ordering::SeqCst));
    drop(connection);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_truncated_upstream_never_becomes_a_trimmed_complete_playlist() {
    let body = vod(180).trim_end_matches("#EXT-X-ENDLIST\n").to_owned();
    for chunk_size in [4096, body.len()] {
        let truncated = &body[..body.len() - 5];
        let origin = Arc::new(PacedPlaylist {
            bytes: Bytes::copy_from_slice(truncated.as_bytes()),
            interval: Duration::ZERO,
            chunk_size: chunk_size.min(truncated.len()),
            finished: Arc::default(),
            fail: true,
            known_length: false,
        });
        let output = Capture::default();
        let (dependencies, _, _) = deps(origin, &output);
        let server = RelayServer::start(RelayConfig::http(0), dependencies)
            .await
            .expect("relay");
        let mut connection = Conn::open(server.port()).await;
        let reply = connection
            .get(&play(
                "truncated",
                "index.m3u8",
                &upstream_url("vod", "index.m3u8"),
            ))
            .await;
        assert_eq!(reply.status, 200);
        assert!(!reply.complete);
        let text = String::from_utf8(reply.body).expect("text");
        assert!(!text.contains("#EXT-X-MEDIA-SEQUENCE:"));
        assert!(!text.contains("#EXT-X-ENDLIST"));
        assert!(!output.logs().contains("playlist trimmed"));
        server.stop().await;
    }
}

struct ReloadingPlaylist {
    bodies: Vec<Bytes>,
    hits: AtomicUsize,
}

impl Upstream for ReloadingPlaylist {
    fn send(&self, _: UpstreamRequest) -> Sending<'_> {
        let index = self.hits.fetch_add(1, Ordering::SeqCst);
        let bytes = self.bodies[index.min(self.bodies.len() - 1)].clone();
        Box::pin(async move {
            let origin = PacedPlaylist {
                chunk_size: bytes.len().div_ceil(31),
                bytes,
                interval: Duration::from_millis(100),
                finished: Arc::default(),
                fail: false,
                known_length: false,
            };
            origin
                .send(UpstreamRequest {
                    method: Method::GET,
                    url: url::Url::parse("https://example.test/index.m3u8").expect("url"),
                    headers: HeaderMap::new(),
                })
                .await
        })
    }
}

fn slow_live(sequence: u64, endlist: bool) -> Bytes {
    let body = vod(2880).replacen(
        "#EXTM3U\n",
        &format!("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n"),
        1,
    );
    Bytes::from(if endlist {
        body
    } else {
        body.replace("#EXT-X-ENDLIST\n", "")
    })
}

async fn remaining_chunks(connection: &mut Conn, mut body: Vec<u8>) -> Vec<u8> {
    loop {
        let size = connection.line().await.expect("chunk size");
        let size = usize::from_str_radix(&size, 16).expect("size");
        assert!(connection.need(size + 2).await);
        body.extend(connection.buf.drain(..size));
        connection.buf.drain(..2);
        if size == 0 {
            return body;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_nonzero_sequence_trims_first_three_second_download() {
    let origin = Arc::new(ReloadingPlaylist {
        bodies: vec![slow_live(1_435_073, false)],
        hits: AtomicUsize::new(0),
    });
    let output = Capture::default();
    let (dependencies, _, _) = deps(origin, &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    let mut connection = Conn::open(server.port()).await;
    let start = Instant::now();
    let reply = slow_reply(
        &mut connection,
        &play(
            "slow-live",
            "index.m3u8",
            &upstream_url("live", "index.m3u8"),
        ),
    )
    .await;
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert!(reply.complete);
    let text = String::from_utf8(reply.body).expect("body");
    assert_eq!(text.matches("#EXTINF:").count(), 12);
    assert!(text.contains("#EXT-X-MEDIA-SEQUENCE:1437941"));
    assert!(text.contains("seg_002879.ts"));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_zero_sequence_is_learned_then_forgotten_after_endlist() {
    let origin = Arc::new(ReloadingPlaylist {
        bodies: vec![
            slow_live(0, false),
            slow_live(0, false),
            slow_live(0, true),
            slow_live(0, false),
        ],
        hits: AtomicUsize::new(0),
    });
    let output = Capture::default();
    let (dependencies, _, _) = deps(origin, &output);
    dependencies.log.set_dev_console(true);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    for (index, expected) in [2880, 12, 2880, 2880].into_iter().enumerate() {
        let mut connection = Conn::open(server.port()).await;
        let target = format!("{}?token={index}", upstream_url("learn", "index.m3u8"));
        let reply = slow_reply(&mut connection, &play("learn-live", "index.m3u8", &target)).await;
        assert!(reply.complete);
        let text = String::from_utf8(reply.body).expect("body");
        assert_eq!(text.matches("#EXTINF:").count(), expected, "reload {index}");
        assert_eq!(text.contains("#EXT-X-ENDLIST"), index == 2);
        assert!(eventually(|| server.in_flight() == 0, WITHIN).await);
    }
    let log = output.logs();
    assert!(log.contains("large-media-playlist"), "{log}");
    assert!(log.contains("untrimmed=budget"), "{log}");
    assert_eq!(log.matches("large-media-playlist").count(), 1, "{log}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_zero_sequence_vod_streams_early_on_every_reload() {
    let origin = Arc::new(ReloadingPlaylist {
        bodies: vec![slow_live(0, true)],
        hits: AtomicUsize::new(0),
    });
    let output = Capture::default();
    let (dependencies, _, _) = deps(origin, &output);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("relay");
    for _ in 0..3 {
        let mut connection = Conn::open(server.port()).await;
        connection
            .send(
                "GET",
                &play(
                    "vod-reload",
                    "index.m3u8",
                    &upstream_url("vod", "index.m3u8"),
                ),
                &[],
            )
            .await;
        let first = tokio::time::timeout(Duration::from_secs(2), first_body_chunk(&mut connection))
            .await
            .expect("early body on every reload");
        let body = remaining_chunks(&mut connection, first).await;
        let text = String::from_utf8(body).expect("body");
        assert_eq!(text.matches("#EXTINF:").count(), 2880);
        assert!(text.contains("#EXT-X-ENDLIST"));
        assert!(eventually(|| server.in_flight() == 0, WITHIN).await);
    }
    server.stop().await;
}

async fn slow_reply(connection: &mut Conn, path: &str) -> Reply {
    connection.send("GET", path, &[]).await;
    tokio::time::timeout(Duration::from_secs(15), connection.read_reply(false))
        .await
        .expect("slow playlist response")
}
