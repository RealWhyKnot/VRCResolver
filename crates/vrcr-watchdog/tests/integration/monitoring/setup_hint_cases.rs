use super::*;
use vrcr_watchdog::monitoring::vrclog::{SetupPass, SetupProbe};

const EXTERNAL_FAILURE: &str = "[AVProVideo] Opening https://media.example.test/video.mp4\n[AVProVideo] Error: Loading failed\n";
const SETUP_HINT: &str = "Type repair to finish setting up videos for public worlds.";

#[test]
fn external_load_failure_points_to_repair_once_until_setup_recovers() {
    let rig = Rig::new();
    let incomplete = Arc::new(Mutex::new(Some(true)));
    let state = Arc::clone(&incomplete);
    let probes = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&probes);
    let mut deps = rig.deps(AudioCheck::new(AudioState::Ok, Some("Speakers")));
    deps.stats = Some(Arc::new(Stats::new(rig.now())));
    deps.setup = Some(SetupProbe {
        pass: Box::new(SetupPass::default),
        incomplete: Box::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
            *state.lock().unwrap_or_else(|error| error.into_inner())
        }),
    });
    let mut monitor = rig.monitor_with(deps);
    let local = "[AVProVideo] Opening https://localhost.youtube.com:57321/play/test/manifest.mp4\n[AVProVideo] Error: Loading failed\n";

    rig.feed(&mut monitor, local);
    assert!(!rig.out.text().contains(SETUP_HINT));
    assert_eq!(probes.load(Ordering::Relaxed), 0);
    let before = rig.out.text().len();
    rig.feed(&mut monitor, EXTERNAL_FAILURE);
    let output = rig.out.text();
    let (row, _) = output[before..].split_once(SETUP_HINT).expect("setup hint");
    assert!(row.contains("Failed"));
    assert!(row.contains("media.example.test"));
    assert!(row.contains("VRChat couldn't load the video."));
    for _ in 0..29 {
        rig.clock.advance(secs(1));
        monitor.poll_once(rig.now());
        rig.feed(&mut monitor, EXTERNAL_FAILURE);
    }
    assert_eq!(probes.load(Ordering::Relaxed), 1);
    assert_eq!(rig.out.text().matches(SETUP_HINT).count(), 1);

    *incomplete.lock().unwrap_or_else(|error| error.into_inner()) = None;
    rig.clock.advance(secs(1));
    monitor.poll_once(rig.now());
    *incomplete.lock().unwrap_or_else(|error| error.into_inner()) = Some(true);
    rig.clock.advance(secs(30));
    rig.feed(&mut monitor, EXTERNAL_FAILURE);
    assert_eq!(probes.load(Ordering::Relaxed), 3);
    assert_eq!(rig.out.text().matches(SETUP_HINT).count(), 1);

    *incomplete.lock().unwrap_or_else(|error| error.into_inner()) = Some(false);
    rig.clock.advance(secs(30));
    monitor.poll_once(rig.now());
    let before = rig.out.text().len();
    rig.feed(&mut monitor, EXTERNAL_FAILURE);
    let output = rig.out.text();
    assert!(!output[before..].contains("repair"));
    assert!(output[before..].contains("VRChat couldn't load the video."));
    *incomplete.lock().unwrap_or_else(|error| error.into_inner()) = Some(true);
    rig.clock.advance(secs(30));
    rig.feed(
        &mut monitor,
        "[AVProVideo] Opening rtsp://media.example.test/live\n[AVProVideo] Error: Loading failed\n",
    );
    assert_eq!(rig.out.text().matches(SETUP_HINT).count(), 2);
}

#[test]
fn running_and_reported_setup_passes_suppress_playback_hints() {
    let rig = Rig::new();
    let pass = Arc::new(Mutex::new(SetupPass {
        running: true,
        ..SetupPass::default()
    }));
    let current = Arc::clone(&pass);
    let probes = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&probes);
    let mut deps = rig.deps(AudioCheck::new(AudioState::Ok, Some("Speakers")));
    deps.setup = Some(SetupProbe {
        pass: Box::new(move || *current.lock().unwrap_or_else(|error| error.into_inner())),
        incomplete: Box::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
            Some(true)
        }),
    });
    let mut monitor = rig.monitor_with(deps);
    rig.feed(&mut monitor, EXTERNAL_FAILURE);
    assert_eq!(probes.load(Ordering::Relaxed), 0);
    assert!(!rig.out.text().contains(SETUP_HINT));

    *pass.lock().unwrap_or_else(|error| error.into_inner()) = SetupPass {
        generation: 1,
        running: false,
        reported: true,
    };
    rig.feed(&mut monitor, EXTERNAL_FAILURE);
    assert_eq!(probes.load(Ordering::Relaxed), 1);
    assert!(!rig.out.text().contains(SETUP_HINT));

    pass.lock()
        .unwrap_or_else(|error| error.into_inner())
        .running = true;
    monitor.poll_once(rig.now());
    rig.feed(&mut monitor, EXTERNAL_FAILURE);
    assert_eq!(probes.load(Ordering::Relaxed), 1);
    assert!(!rig.out.text().contains(SETUP_HINT));

    *pass.lock().unwrap_or_else(|error| error.into_inner()) = SetupPass {
        generation: 2,
        ..SetupPass::default()
    };
    monitor.poll_once(rig.now());
    rig.feed(&mut monitor, EXTERNAL_FAILURE);
    assert_eq!(probes.load(Ordering::Relaxed), 2);
    assert_eq!(rig.out.text().matches(SETUP_HINT).count(), 1);
}
