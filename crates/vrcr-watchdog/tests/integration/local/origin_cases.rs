use super::*;

const WITHIN: Duration = Duration::from_secs(10);

async fn play(local: &Local) -> Result<local::Play, &'static str> {
    tokio::time::timeout(WITHIN, local.play(ask()))
        .await
        .unwrap_or_else(|_| panic!("no answer within {WITHIN:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_video_resolves_to_a_manifest_the_relay_can_serve() {
    let origin = FakeOrigin::start(Mode::Plays).await;
    let manifests = Arc::new(Manifests::default());
    let local = origin.local(&manifests);
    assert!(local.covers(WATCH));
    let played = play(&local).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(played.height, 1080);
    assert_eq!(played.video_codec, "avc1.64002A");
    assert_eq!(played.audio_codec.as_deref(), Some("mp4a.40.2"));
    let path = played
        .url
        .strip_prefix(RELAY)
        .unwrap_or_else(|| panic!("{}", played.url));
    let token = local::token(path).unwrap_or_else(|| panic!("{path}"));
    let manifest = manifests
        .get(token)
        .unwrap_or_else(|| panic!("manifest not stored"));
    let media = format!("http://127.0.0.1:{}/hls", origin.port);
    assert!(
        manifest.contains(&format!("\n{media}/v1080p60.m3u8\n")),
        "{manifest}"
    );
    assert!(
        manifest.contains(&format!("URI=\"{media}/audio/en.m3u8\"")),
        "{manifest}"
    );
    assert_eq!(manifest.matches("#EXT-X-STREAM-INF").count(), 1);
    assert_eq!(manifest.matches("#EXT-X-MEDIA").count(), 1);
    assert_eq!(
        origin.hits(),
        [
            "/sw.js_data",
            "/youtubei/v1/player?prettyPrint=false",
            "/hls/master.m3u8",
            "/hls/v1080p60.m3u8",
            "/hls/seg0.ts",
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_visitor_is_fetched_once_and_each_play_gets_its_own_manifest() {
    let origin = FakeOrigin::start(Mode::Plays).await;
    let manifests = Arc::new(Manifests::default());
    let local = origin.local(&manifests);
    let first = play(&local).await.unwrap_or_else(|e| panic!("{e}"));
    let second = play(&local).await.unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(first.url, second.url);
    let visitor_hits = origin
        .hits()
        .iter()
        .filter(|path| *path == "/sw.js_data")
        .count();
    assert_eq!(visitor_hits, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_height_limit_reaches_the_variant_check() {
    let origin = FakeOrigin::start(Mode::Plays).await;
    let local = origin.local(&Arc::new(Manifests::default()));
    let played = tokio::time::timeout(
        WITHIN,
        local.play(Ask {
            url: WATCH,
            max_height: Some(720),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("no answer within {WITHIN:?}"))
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(played.height, 720);
    assert!(origin.hits().contains(&"/hls/v720.m3u8".to_owned()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sign_in_wall_is_refused_and_the_visitor_is_fetched_again() {
    let origin = FakeOrigin::start(Mode::SignInWall).await;
    let local = origin.local(&Arc::new(Manifests::default()));
    assert_eq!(play(&local).await, Err(youtube::SIGN_IN_WALL));
    origin.is_walled.store(false, Ordering::SeqCst);
    assert_eq!(play(&local).await.map(|played| played.height), Ok(1080));
    let visitor_hits = origin
        .hits()
        .iter()
        .filter(|path| *path == "/sw.js_data")
        .count();
    assert_eq!(visitor_hits, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_dead_end_is_refused_with_its_reason() {
    let cases = [
        (Mode::Live, "live"),
        (Mode::NoManifest, "no_hls"),
        (Mode::ForeignManifest, "foreign_host"),
        (Mode::Vp9Only, "no_h264"),
        (Mode::ForeignAudio, "foreign_host"),
        (Mode::SegmentMissing, "segment_failed"),
        (Mode::VisitorDown, "visitor_failed"),
    ];
    for (mode, reason) in cases {
        let origin = FakeOrigin::start(mode).await;
        let local = origin.local(&Arc::new(Manifests::default()));
        assert_eq!(play(&local).await, Err(reason));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resting_step_refuses_without_asking_anything() {
    let origin = FakeOrigin::start(Mode::Plays).await;
    let manifests = Arc::new(Manifests::default());
    let local = origin.local(&manifests);
    for _ in 0..FAILURES_BEFORE_REST {
        let played = play(&local).await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(manifests.failed(&played.url).as_deref(), Some(WATCH));
    }
    let asked = origin.hits().len();
    assert_eq!(play(&local).await, Err("resting"));
    assert_eq!(origin.hits().len(), asked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_site_that_keeps_failing_rests_the_step() {
    let origin = FakeOrigin::start(Mode::VisitorDown).await;
    let manifests = Arc::new(Manifests::default());
    let local = origin.local(&manifests);
    for _ in 0..FAULTS_BEFORE_REST {
        assert_eq!(play(&local).await, Err("visitor_failed"));
    }
    let asked = origin.hits().len();
    assert_eq!(play(&local).await, Err("resting"));
    assert_eq!(origin.hits().len(), asked);
    assert!(manifests.is_resting());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusals_about_one_video_never_rest_the_step() {
    let origin = FakeOrigin::start(Mode::Live).await;
    let manifests = Arc::new(Manifests::default());
    let local = origin.local(&manifests);
    for _ in 0..FAULTS_BEFORE_REST * 2 {
        assert_eq!(play(&local).await, Err("live"));
    }
    assert!(!manifests.is_resting());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandoned_attempt_counts_as_a_fault() {
    let origin = FakeOrigin::start(Mode::Plays).await;
    let manifests = Arc::new(Manifests::default());
    let local = origin.local(&manifests);
    for _ in 0..FAULTS_BEFORE_REST {
        drop(local.play(ask()));
    }
    assert!(!manifests.is_resting());
    for _ in 0..FAULTS_BEFORE_REST {
        let mut attempt = local.play(ask());
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        assert!(attempt.as_mut().poll(&mut context).is_pending());
        drop(attempt);
    }
    assert!(manifests.is_resting());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_asked_when_the_relay_is_down_or_the_link_is_foreign() {
    let origin = FakeOrigin::start(Mode::Plays).await;
    let manifests = Arc::new(Manifests::default());
    let local = origin.local_behind(&manifests, None);
    assert_eq!(play(&local).await, Err("relay_down"));
    let local = origin.local(&manifests);
    assert!(!local.covers("https://example.com/v"));
    let refused = tokio::time::timeout(
        WITHIN,
        local.play(Ask {
            url: "https://example.com/v",
            max_height: None,
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("no answer within {WITHIN:?}"));
    assert_eq!(refused, Err("not_youtube"));
    assert_eq!(origin.hits(), Vec::<String>::new());
}
