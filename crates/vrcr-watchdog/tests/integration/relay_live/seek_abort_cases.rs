use super::*;
use std::collections::HashMap;

const SEGMENT_BYTES: usize = 313_208;
const FIELD_ABORT_BYTES: usize = 31_887;

#[derive(Default)]
struct SeekOrigin {
    hits: Mutex<HashMap<String, usize>>,
    pending: Arc<AtomicUsize>,
    bodies: Arc<AtomicUsize>,
    abort_bytes: AtomicUsize,
}

impl SeekOrigin {
    fn hits(&self, path: &str) -> usize {
        *self
            .hits
            .lock()
            .expect("valid seek fixture")
            .get(path)
            .unwrap_or(&0)
    }
}

impl Upstream for SeekOrigin {
    fn send(&self, request: UpstreamRequest) -> Sending<'_> {
        Box::pin(async move {
            let pending = OpenGuard::new(&self.pending);
            let path = request.url.path();
            let hit = {
                let mut hits = self.hits.lock().expect("valid seek fixture");
                let count = hits.entry(path.to_owned()).or_default();
                *count += 1;
                let hit = *count;
                drop(hits);
                hit
            };
            let file = path.rsplit('/').next().expect("valid seek fixture");
            let manifest = match file {
                "master.m3u8" => {
                    Some("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1200000\nindex.m3u8\n".to_owned())
                }
                "index.m3u8" => {
                    let mut playlist = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n".to_owned();
                    for segment in 0..29 {
                        writeln!(playlist, "#EXTINF:2,\nseg_{segment:06}.ts")
                            .expect("valid seek fixture");
                    }
                    playlist.push_str("#EXT-X-ENDLIST\n");
                    Some(playlist)
                }
                _ => None,
            };
            if let Some(manifest) = manifest {
                return Ok(respond(
                    &self.bodies,
                    200,
                    &[("content-type", "application/vnd.apple.mpegurl".to_owned())],
                    vec![Ok(Bytes::from(manifest))],
                    Tail::End,
                ));
            }
            assert_eq!(request.headers["range"], "bytes=0-");
            if file == "seg_000000.ts" && hit == 2 {
                std::future::pending::<()>().await;
            }
            drop(pending);
            let segment: u8 = file
                .strip_prefix("seg_")
                .expect("valid seek fixture")
                .strip_suffix(".ts")
                .expect("valid seek fixture")
                .parse()
                .expect("valid seek fixture");
            let partial = segment == 0 && hit == 3;
            Ok(respond(
                &self.bodies,
                206,
                &[
                    ("content-type", "video/mp2t".to_owned()),
                    ("content-length", SEGMENT_BYTES.to_string()),
                    ("content-range", format!("bytes 0-313207/{SEGMENT_BYTES}")),
                ],
                filled(
                    if partial {
                        self.abort_bytes.load(Ordering::SeqCst)
                    } else {
                        SEGMENT_BYTES
                    },
                    segment,
                ),
                if partial { Tail::Pending } else { Tail::End },
            ))
        })
    }
}

fn next_value(seed: &mut u64, limit: u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (*seed >> 32) % limit
}

async fn full_segment(connection: &mut Conn, path: &str, segment: u8) {
    tokio::time::timeout(WITHIN, async {
        connection.send("GET", path, &[("Range", "bytes=0-")]).await;
        let reply = connection.reply(false).await;
        assert_eq!(reply.status, 206, "{path}");
        assert_eq!(reply.header("content-length"), Some("313208"));
        assert_eq!(reply.header("content-range"), Some("bytes 0-313207/313208"));
        assert!(reply.complete, "{path}");
        assert_eq!(reply.body, vec![segment; SEGMENT_BYTES], "{path}");
    })
    .await
    .unwrap_or_else(|_| panic!("segment stalled: {path}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seek_aborts_leave_followup_segments_prompt_and_complete() {
    let origin = Arc::new(SeekOrigin::default());
    let output = Capture::default();
    let (dependencies, stats, _) = deps(Arc::<SeekOrigin>::clone(&origin), &output);
    dependencies.log.set_dev_console(true);
    let server = RelayServer::start(RelayConfig::http(0), dependencies)
        .await
        .expect("valid seek fixture");
    let mut seed = 0x5eed_2026_1109_u64;
    for reuse in [false, true] {
        for round in 0..80 {
            let play_id = format!("seek-{}-{round}", usize::from(reuse));
            let upstream_prefix = format!("/api/proxy/lazy-hls/{play_id}");
            let local_prefix = format!("/play/{play_id}");
            let segment_zero = format!("{local_prefix}/seg_000000.ts");
            let upstream_zero = format!("{upstream_prefix}/seg_000000.ts");
            let abort_bytes = if round == 0 {
                FIELD_ABORT_BYTES
            } else {
                28_000 + usize::try_from(next_value(&mut seed, 8000)).expect("valid seek fixture")
            };
            origin.abort_bytes.store(abort_bytes, Ordering::SeqCst);
            let mut connection = Conn::open(server.port()).await;
            let master = connection
                .get(&play(
                    &play_id,
                    "master.m3u8",
                    &format!("https://us1.vrcresolver.com{upstream_prefix}/master.m3u8"),
                ))
                .await;
            assert_eq!(master.status, 200);
            assert!(master.complete);
            assert!(String::from_utf8_lossy(&master.body).contains("index.m3u8"));
            if !reuse {
                connection = Conn::open(server.port()).await;
            }
            let playlist = connection.get(&format!("{local_prefix}/index.m3u8")).await;
            assert_eq!(playlist.status, 200);
            assert!(playlist.complete);
            assert_eq!(
                String::from_utf8_lossy(&playlist.body)
                    .matches("#EXTINF:")
                    .count(),
                29
            );
            for segment in 0..14 {
                if !reuse {
                    connection = Conn::open(server.port()).await;
                }
                full_segment(
                    &mut connection,
                    &format!("{local_prefix}/seg_{segment:06}.ts"),
                    segment,
                )
                .await;
            }

            let mut before_headers = Conn::open(server.port()).await;
            before_headers
                .send("GET", &segment_zero, &[("Range", "bytes=0-")])
                .await;
            assert!(eventually(|| origin.hits(&upstream_zero) == 2, WITHIN).await);
            let mut response_byte = [0];
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(if round == 0 {
                        15
                    } else {
                        next_value(&mut seed, 20)
                    }),
                    before_headers.stream.read(&mut response_byte),
                )
                .await
                .is_err()
            );
            drop(before_headers);
            tokio::time::sleep(Duration::from_millis(if round == 0 {
                18
            } else {
                next_value(&mut seed, 24)
            }))
            .await;

            let mut after_headers = Conn::open(server.port()).await;
            after_headers
                .send("GET", &segment_zero, &[("Range", "bytes=0-")])
                .await;
            let headers = after_headers.reply(true).await;
            assert_eq!(headers.status, 206);
            assert_eq!(headers.header("content-length"), Some("313208"));
            assert!(
                tokio::time::timeout(WITHIN, after_headers.need(abort_bytes))
                    .await
                    .expect("valid seek fixture")
            );
            assert_eq!(after_headers.buf, vec![0; abort_bytes]);
            assert_eq!(origin.bodies.load(Ordering::SeqCst), 1);
            tokio::time::sleep(Duration::from_millis(if round == 0 {
                2
            } else {
                next_value(&mut seed, 5)
            }))
            .await;
            drop(after_headers);
            tokio::time::sleep(Duration::from_millis(if round == 0 {
                10
            } else {
                next_value(&mut seed, 15)
            }))
            .await;

            for segment in 0..2 {
                if !reuse {
                    connection = Conn::open(server.port()).await;
                }
                full_segment(
                    &mut connection,
                    &format!("{local_prefix}/seg_{segment:06}.ts"),
                    segment,
                )
                .await;
            }
            assert!(
                eventually(
                    || server.in_flight() == 0
                        && origin.pending.load(Ordering::SeqCst) == 0
                        && origin.bodies.load(Ordering::SeqCst) == 0,
                    WITHIN
                )
                .await
            );
            for segment in 0..29 {
                let expected = match segment {
                    0 => 4,
                    1 => 2,
                    2..14 => 1,
                    _ => 0,
                };
                assert_eq!(
                    origin.hits(&format!("{upstream_prefix}/seg_{segment:06}.ts")),
                    expected
                );
            }
            assert_eq!(origin.hits(&format!("{upstream_prefix}/master.m3u8")), 1);
            assert_eq!(origin.hits(&format!("{upstream_prefix}/index.m3u8")), 1);
        }
    }
    assert_eq!(stats.activity().upstream_relay_requests, 160 * 20);
    let diagnostics = output.text();
    assert_eq!(
        diagnostics.matches("segment-client_disconnect").count(),
        320
    );
    assert!(diagnostics.contains("segment-backtrack"), "{diagnostics}");
    assert!(diagnostics.contains("segment-retry"), "{diagnostics}");
    tokio::time::timeout(WITHIN, server.stop())
        .await
        .expect("valid seek fixture");
}
