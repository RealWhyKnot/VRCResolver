use vrcr_watchdog::local::store::{FAILURES_BEFORE_REST, Manifests};

use super::*;

const WATCH: &str = "https://www.youtube.com/watch?v=aqz-KE-bpKQ";

struct LocalRig {
    rig: Rig,
    plays: Arc<Manifests>,
    monitor: VrcLogMonitor,
}

impl LocalRig {
    fn new() -> LocalRig {
        let rig = Rig::new().with_cache().with_gate();
        let plays = Arc::new(Manifests::default());
        let mut deps = rig.deps(AudioCheck::new(AudioState::Ok, Some("Speakers")));
        deps.local = Some(Arc::clone(&plays));
        let monitor = rig.monitor_with(deps);
        LocalRig {
            rig,
            plays,
            monitor,
        }
    }

    fn open(&mut self) -> String {
        let token = self.plays.insert("#EXTM3U\n".to_owned(), WATCH);
        let url = format!("https://localhost.youtube.com:4443/play/local-{token}/master.m3u8");
        self.rig
            .feed(&mut self.monitor, &format!("[AVProVideo] Opening {url}\n"));
        self.rig.clock.advance(secs(2));
        url
    }

    fn fail(&mut self) {
        self.open();
        self.rig
            .feed(&mut self.monitor, "[AVProVideo] Error: Loading failed\n");
    }

    fn prefers_og(&self) -> bool {
        self.rig
            .hint
            .as_ref()
            .is_some_and(|hint| hint.should_prefer_og(WATCH))
    }
}

#[test]
fn a_failed_local_load_sends_the_next_try_to_the_stock_resolver() {
    let mut r = LocalRig::new();
    assert!(!r.prefers_og());
    r.fail();
    assert!(r.prefers_og());
    assert!(!r.plays.is_resting());
    let text = r.rig.out.text();
    assert!(text.contains("load_failure ms=2000 url="), "{text}");
    assert!(text.contains(" og_hint=armed"), "{text}");
}

#[test]
fn a_stalled_local_load_sends_the_next_try_to_the_stock_resolver() {
    let mut r = LocalRig::new();
    r.open();
    r.rig.tick(&mut r.monitor, secs(300));
    assert!(r.prefers_og());
    let text = r.rig.out.text();
    assert!(text.contains("silent_stall ms="), "{text}");
}

#[test]
fn local_plays_never_reach_the_server_or_its_health_gate() {
    let mut r = LocalRig::new();
    for _ in 0..health::OPEN_THRESHOLD + FAILURES_BEFORE_REST {
        r.fail();
    }
    r.open();
    r.rig.feed(&mut r.monitor, "fwidth=1920 fheight=1080\n");
    r.rig.tick(&mut r.monitor, secs(60));
    assert_eq!(r.rig.sink.all(), Vec::new());
    assert!(!r.rig.gate_open());
}

#[test]
fn two_failed_local_loads_in_a_row_rest_the_local_step() {
    let mut r = LocalRig::new();
    for _ in 1..FAILURES_BEFORE_REST {
        r.fail();
    }
    assert!(!r.plays.is_resting());
    r.fail();
    assert!(r.plays.is_resting());
}

#[test]
fn a_local_play_that_starts_clears_the_failure_count() {
    let mut r = LocalRig::new();
    for _ in 1..FAILURES_BEFORE_REST {
        r.fail();
    }
    r.open();
    r.rig
        .feed(&mut r.monitor, "[AVProVideo] Using playback path: x\n");
    for _ in 1..FAILURES_BEFORE_REST {
        r.fail();
    }
    assert!(!r.plays.is_resting());
    r.open();
    r.rig.feed(&mut r.monitor, "fwidth=1920 fheight=1080\n");
    for _ in 1..FAILURES_BEFORE_REST {
        r.fail();
    }
    assert!(!r.plays.is_resting());
}

#[test]
fn a_local_address_the_watchdog_never_issued_changes_nothing() {
    let mut r = LocalRig::new();
    for _ in 0..health::OPEN_THRESHOLD {
        r.rig.feed(
            &mut r.monitor,
            "[AVProVideo] Opening https://localhost.youtube.com:4443/play/local-unknown/master.m3u8\n",
        );
        r.rig
            .feed(&mut r.monitor, "[AVProVideo] Error: Loading failed\n");
    }
    assert!(!r.prefers_og());
    assert!(!r.plays.is_resting());
    assert!(!r.rig.gate_open());
    assert_eq!(r.rig.sink.all(), Vec::new());
}
