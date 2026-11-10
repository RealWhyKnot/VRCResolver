use std::sync::atomic::{AtomicUsize, Ordering};

use vrcr_watchdog::local::{self, Ask, Attempt, Play};

use super::*;

const YT: &str = "https://www.youtube.com/watch?v=aqz-KE-bpKQ";
const LOCAL_MEDIA: &str = "https://localhost.youtube.com:4443/play/local-t1/master.m3u8";

enum Answer {
    Plays,
    Refuses,
    Hangs,
}

struct FakeStep {
    answer: Answer,
    calls: AtomicUsize,
    heights: Mutex<Vec<Option<i32>>>,
}

impl FakeStep {
    fn new(answer: Answer) -> Arc<FakeStep> {
        Arc::new(FakeStep {
            answer,
            calls: AtomicUsize::new(0),
            heights: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl local::Step for FakeStep {
    fn covers(&self, url: &str) -> bool {
        url.contains("youtube.com")
    }

    fn play<'a>(&'a self, ask: Ask<'a>) -> Attempt<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.heights
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(ask.max_height);
        Box::pin(async move {
            match self.answer {
                Answer::Plays => Ok(Play {
                    url: LOCAL_MEDIA.to_owned(),
                    height: 1080,
                    video_codec: "avc1.640028".to_owned(),
                    audio_codec: Some("mp4a.40.2".to_owned()),
                }),
                Answer::Refuses => Err("sign_in_wall"),
                Answer::Hangs => std::future::pending().await,
            }
        })
    }
}

fn yt(player: &str, extra: &str) -> String {
    format!(
        "{{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"{YT}\",\"player\":\"{player}\"{extra}}}"
    )
}

fn with_step(mesh: &Arc<FakeMesh>, step: &Arc<FakeStep>) -> Arc<Ipc<FakeMesh>> {
    Ipc::builder(Arc::clone(mesh), Ux::sink(), Log::disabled())
        .local(Some(Arc::<FakeStep>::clone(step)))
        .build()
}

#[tokio::test(start_paused = true)]
async fn a_server_fallback_is_answered_locally() {
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let step = FakeStep::new(Answer::Plays);
    let reported = Arc::new(Mutex::new(Vec::new()));
    let ipc = Ipc::builder(Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .local(Some(Arc::<FakeStep>::clone(&step)))
        .report_fallback({
            let reported = Arc::clone(&reported);
            move |_, reason| {
                reported
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(reason.to_owned());
            }
        })
        .build();
    let v = ask(&ipc, &yt("avpro", "")).await;
    assert_eq!(v["action"], action::RESOLVED);
    assert_eq!(v["id"], "x1");
    assert_eq!(v["url"], LOCAL_MEDIA);
    assert_eq!(v["engine"], local::ENGINE);
    assert_eq!(v["protocol"], "hls");
    assert_eq!(v["video_codec"], "h264");
    assert_eq!(v["audio_codec"], "aac");
    assert_eq!(v["resolved_height"], 1080);
    assert_eq!(step.calls(), 1);
    assert_eq!(
        *reported.lock().unwrap_or_else(|e| e.into_inner()),
        Vec::<String>::new()
    );
}

#[tokio::test(start_paused = true)]
async fn a_server_answer_never_reaches_the_local_step() {
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let step = FakeStep::new(Answer::Plays);
    let v = ask(&with_step(&mesh, &step), &yt("avpro", "")).await;
    assert_eq!(v["url"], MESH_MEDIA);
    assert_eq!(step.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_refusal_forwards_the_server_fallback_unchanged() {
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let plain = Ipc::builder(Arc::clone(&mesh), Ux::sink(), Log::disabled()).build();
    let expected = exchange(&plain, &yt("avpro", "")).await;
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let step = FakeStep::new(Answer::Refuses);
    let got = exchange(&with_step(&mesh, &step), &yt("avpro", "")).await;
    assert_eq!(got, expected);
    assert_eq!(step.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_hung_local_step_gives_way_to_the_fallback_inside_its_budget() {
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let step = FakeStep::new(Answer::Hangs);
    let started = Instant::now();
    let v = ask(&with_step(&mesh, &step), &yt("avpro", "")).await;
    assert_eq!(v["action"], action::FALLBACK_NATIVE);
    assert_eq!(reason(&v), fallback::ALL_CONFIGS_FAILED);
    assert_eq!(started.elapsed(), local::MAX_BUDGET);
}

#[tokio::test(start_paused = true)]
async fn unity_and_other_sites_skip_the_local_step() {
    let step = FakeStep::new(Answer::Plays);
    let mesh = FakeMesh::always(fallback_step(fallback::ALL_CONFIGS_FAILED));
    let ipc = with_step(&mesh, &step);
    assert_eq!(
        ask(&ipc, &yt("unity", "")).await["action"],
        action::FALLBACK_NATIVE
    );
    assert_eq!(
        ask(&ipc, &req("avpro", "")).await["action"],
        action::FALLBACK_NATIVE
    );
    assert_eq!(step.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_mesh_error_is_answered_locally() {
    let mesh = FakeMesh::connected(vec![Step::Fail]);
    let step = FakeStep::new(Answer::Plays);
    let v = ask(&with_step(&mesh, &step), &yt("avpro", "")).await;
    assert_eq!(v["url"], LOCAL_MEDIA);
}

#[tokio::test(start_paused = true)]
async fn a_spent_deadline_goes_straight_to_the_fallback() {
    let mesh = FakeMesh::always(Step::Hang);
    let step = FakeStep::new(Answer::Plays);
    let v = ask(&with_step(&mesh, &step), &yt("avpro", "")).await;
    assert_eq!(reason(&v), fallback::CLIENT_DEADLINE_EXCEEDED);
    assert_eq!(step.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_short_wrapper_deadline_skips_the_local_step() {
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let step = FakeStep::new(Answer::Plays);
    let v = ask(
        &with_step(&mesh, &step),
        &yt("avpro", ",\"wrapper_deadline_ms\":1200"),
    )
    .await;
    assert_eq!(reason(&v), fallback::ALL_CONFIGS_FAILED);
    assert_eq!(step.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_paused_resolver_is_answered_locally() {
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let step = FakeStep::new(Answer::Plays);
    let ipc = Ipc::builder(Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .health(open_gate(vrcr_watchdog::clock::system()))
        .local(Some(Arc::<FakeStep>::clone(&step)))
        .build();
    assert_eq!(ask(&ipc, &yt("avpro", "")).await["url"], LOCAL_MEDIA);
    assert_eq!(mesh.resolves().len(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_paused_resolver_still_falls_back_when_the_local_step_refuses() {
    let step = FakeStep::new(Answer::Refuses);
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled())
        .health(open_gate(vrcr_watchdog::clock::system()))
        .local(Some(Arc::<FakeStep>::clone(&step)))
        .build();
    assert_eq!(
        reason(&ask(&ipc, &yt("avpro", "")).await),
        og_reason::RESOLVER_UNHEALTHY
    );
    assert_eq!(step.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_failed_load_sends_the_next_attempt_to_the_game_resolver() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(YT);
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let step = FakeStep::new(Answer::Plays);
    let ipc = Ipc::builder(Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .og_hint(hint)
        .local(Some(Arc::<FakeStep>::clone(&step)))
        .build();
    assert_eq!(
        reason(&ask(&ipc, &yt("avpro", "")).await),
        og_reason::PRIOR_LOAD_FAILURE
    );
    assert_eq!(step.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn local_answers_are_not_cached() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let mesh = FakeMesh::always(fallback_step(fallback::ALL_CONFIGS_FAILED));
    let step = FakeStep::new(Answer::Plays);
    let ipc = Ipc::builder(Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(seeded_cache(&dir))
        .local(Some(Arc::<FakeStep>::clone(&step)))
        .build();
    assert_eq!(ask(&ipc, &yt("avpro", "")).await["url"], LOCAL_MEDIA);
    assert_eq!(ask(&ipc, &yt("avpro", "")).await["url"], LOCAL_MEDIA);
    assert_eq!(mesh.resolves().len(), 2);
    assert_eq!(step.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_local_step_gets_the_capped_height() {
    let mesh = FakeMesh::connected(vec![fallback_step(fallback::ALL_CONFIGS_FAILED)]);
    let step = FakeStep::new(Answer::Plays);
    ask(&with_step(&mesh, &step), &yt("avpro", ",\"maxHeight\":720")).await;
    assert_eq!(
        *step.heights.lock().unwrap_or_else(|e| e.into_inner()),
        vec![Some(720)]
    );
}
