use super::*;

#[test]
fn alternating_logs_do_not_replay_old_lines() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let sessions = Arc::new(AtomicU32::new(0));
    let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
    let counter = std::sync::Arc::clone(&sessions);
    deps.on_new_session = Some(Box::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    let mut m = r.monitor_with(deps);
    let (a1, a2, b1) = (
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a1",
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a2",
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b1",
    );
    let step = |m: &mut VrcLogMonitor| {
        r.clock.advance(secs(10));
        m.poll_once(r.now());
    };
    let count = || sessions.load(Ordering::SeqCst);

    let a = logs.write(
        "output_log_a.txt",
        "2026.10.11 10:00:00 Debug      -  boot\n",
    );
    step(&mut m);
    assert_eq!(count(), 0);
    logs.append(&a, &format!("{}\n", opening(a1)));
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(a1), 0));
    let b = logs.write("output_log_b.txt", &format!("{}\n", opening(b1)));
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(b1), 1));
    logs.touch(&a);
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(b1), 1));
    logs.touch(&b);
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(b1), 1));
    logs.append(&a, &format!("{}\n", opening(a2)));
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(a2), 1));
    logs.touch(&b);
    step(&mut m);
    assert_eq!((m.last_opened_url(), count()), (Some(a2), 1));
}

#[test]
fn load_failure_arms_the_og_hint_before_evicting_the_cache() {
    let r = Rig::new().with_cache();
    let mut m = r.monitor();
    r.store(PLAYBACK);
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(ms(2500));
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    let hint = r.hint.as_ref().unwrap_or_else(|| panic!("hint"));
    assert!(hint.should_prefer_og(SOURCE));
    assert!(!r.cached(PLAYBACK));
    assert_eq!(
        r.sink.all(),
        vec![(PLAYBACK.to_owned(), feedback::LOAD_FAILURE, 2500, None)]
    );
    let text = r.out.logs();
    assert_eq!(
        r.out.text(),
        format!(
            "  Failed     {:<28}VRChat couldn't load the video.\n",
            "us1.vrcresolver.com"
        )
    );
    assert!(text.contains("load_failure ms=2500 url="), "{text}");
    assert!(text.contains(" evicted=1 og_hint=armed"), "{text}");
    assert!(!m.stall_armed());
}

#[test]
fn a_late_load_failure_is_not_attributed_to_the_old_open() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(ms(10_001));
    r.feed(&mut m, "[AVProVideo] Error: Loading failed\n");
    assert_eq!(
        r.sink.all(),
        <[(std::string::String, &str, i32, std::option::Option<i32>); 0]>::default()
    );
    assert!(!r.out.logs().contains("load_failure"));
    assert_eq!(m.last_opened_url(), Some(PLAYBACK));
    assert!(!m.stall_armed());
}

#[test]
fn mark_playback_failure_arms_the_og_hint_for_the_stall_path() {
    let r = Rig::new().with_cache();
    let m = r.monitor();
    r.store(PLAYBACK);
    let recovery = m.mark_playback_failure(PLAYBACK);
    assert_eq!(recovery.evicted, 1);
    assert!(recovery.og_hint_armed);
    assert!(r.hint.as_ref().is_some_and(|h| h.should_prefer_og(SOURCE)));
    assert!(!r.cached(PLAYBACK));
}

#[test]
fn leaving_the_world_mid_load_is_not_a_failed_play() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");
    r.feed(
        &mut m,
        "2026.09.29 13:23:56 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c (offset 0) with API MediaFoundation\n\
         2026.09.29 13:23:59 Debug      -  [Behaviour] OnLeftRoom\n\
         2026.09.29 13:24:00 Error      -  [AVProVideo] Error: Loading failed.  File not found, codec not supported, video resolution too high or insufficient system resources.\n",
    );
    r.tick(&mut m, secs(400));
    assert!(!r.gate_open());
    assert_eq!(
        r.sink.all(),
        <[(std::string::String, &str, i32, std::option::Option<i32>); 0]>::default()
    );
    assert!(m.next_deadline().is_none());
    assert!(
        r.file_log()
            .contains("[vrclog] stopped watching us1.vrcresolver.com reason=world_change")
    );
}

#[test]
fn a_stalled_play_in_the_same_world_still_counts() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");
    r.feed(
        &mut m,
        "2026.09.29 13:24:01 Debug      -  [Behaviour] Entering Room: Hazy Glow\n\
         2026.09.29 13:24:16 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c (offset 0) with API MediaFoundation\n",
    );
    assert_eq!(m.next_deadline(), Some(r.now() + secs(300)));
    r.tick(&mut m, secs(299));
    assert!(!r.gate_open());
    r.tick(&mut m, secs(1));
    assert!(r.gate_open());
    assert_eq!(
        r.sink.all(),
        vec![(
            "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=c".to_owned(),
            feedback::SILENT_STALL,
            300_000,
            None
        )]
    );
    assert_eq!(
        r.out.text(),
        format!(
            "  Stalled    {:<28}The video stopped loading in VRChat.\n  Too many videos failed in a row. VRChat plays videos on its own until the server is back.\n",
            "us1.vrcresolver.com"
        )
    );
    assert!(r.out.logs().contains("silent_stall ms=300000 url="));
}

#[test]
fn progress_lines_disarm_the_stall_watchdog() {
    for line in [
        "Now Playing: something",
        "Load Url: x",
        "[AVProVideo] Using playback path: MF",
    ] {
        let r = Rig::new();
        let mut m = r.monitor();
        r.feed(
            &mut m,
            &format!("[AVProVideo] Opening {PLAYBACK}\n{line}\n"),
        );
        assert!(!m.stall_armed(), "{line}");
        r.tick(&mut m, secs(600));
        assert!(r.sink.all().is_empty(), "{line}");
    }
}

#[test]
fn playing_feedback_repeats_while_the_height_is_fresh() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(secs(2));
    r.feed(&mut m, "fwidth=1280 fheight=720\n");
    assert_eq!(m.playing_feedback_url(), Some(PLAYBACK));
    r.tick(&mut m, secs(14));
    assert_eq!(r.sink.kinds().len(), 1);
    r.tick(&mut m, secs(1));
    r.tick(&mut m, secs(15));
    let all = r.sink.all();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all[1],
        (PLAYBACK.to_owned(), feedback::PLAYING, 17_000, Some(720))
    );
    assert_eq!(all[2].2, 32_000);

    r.feed(
        &mut m,
        "[AVProVideo] Opening https://cdn.example.com/other.mp4\n",
    );
    assert_eq!(m.playing_feedback_url(), None);
    r.feed(&mut m, "Now Playing: other\n");
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    assert_eq!(
        m.playing_feedback_url(),
        Some(PLAYBACK),
        "a remembered height restarts the loop"
    );
    r.feed(&mut m, "Now Playing: again\n");
    r.tick(&mut m, vrclog::DELIVERED_HEIGHT_TTL);
    assert_eq!(
        m.playing_feedback_url(),
        None,
        "an expired height ends the loop"
    );
    let before = r.sink.all().len();
    r.tick(&mut m, secs(60));
    assert_eq!(r.sink.all().len(), before);
}

#[test]
fn a_wedged_relay_is_handed_to_the_bypass_instead_of_counting() {
    for handled in [true, false] {
        let r = Rig::new().with_gate();
        let activity = Arc::new(RelayActivity::new());
        let ports = Arc::new(Mutex::new(Vec::new()));
        let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
        let seen = std::sync::Arc::clone(&ports);
        deps.relay_wedge = Some(RelayWedge {
            activity: std::sync::Arc::clone(&activity),
            on_wedged: Box::new(move |p| {
                seen.lock().unwrap_or_else(|e| e.into_inner()).push(p);
                handled
            }),
        });
        let mut m = r.monitor_with(deps);
        let local =
            format!("http://localhost.youtube.com:51234/play/p{handled}/manifest.m3u8?target=x");
        m.mark_playback_failure(PLAYBACK);
        m.mark_playback_failure(PLAYBACK);
        r.feed(&mut m, &format!("[AVProVideo] Opening {local}\n"));
        r.tick(&mut m, secs(300));
        assert_eq!(
            *ports.lock().unwrap_or_else(|e| e.into_inner()),
            vec![51234]
        );
        let text = r.out.logs();
        assert!(text.contains(
            "relay_pool_wedged ms=300000 port=51234: VRChat never reached the local relay"
        ));
        assert_eq!(text.contains("; counting as a failed play"), !handled);
        assert_eq!(
            r.sink.kinds(),
            if handled {
                vec![]
            } else {
                vec![feedback::SILENT_STALL]
            }
        );

        activity.record(&format!("/play/p{handled}/seg1.ts"));
        assert!(!m.handle_wedged_relay(&local, 1));
    }
}

#[test]
fn load_failures_are_only_blamed_when_sound_output_works() {
    for (no_devices, state, blamed) in [
        (true, AudioState::InUse, false),
        (true, AudioState::NoDevice, false),
        (true, AudioState::Ok, true),
        (true, AudioState::Unknown, true),
        (false, AudioState::InUse, true),
    ] {
        let r = Rig::new().with_cache().with_gate();
        let mut m = r.monitor_with(r.deps(AudioCheck::new(
            state,
            Some("Speakers (3- fifine Microphone)"),
        )));
        if no_devices {
            r.feed(
                &mut m,
                "uSpeak: SetInputDevice 0 (0 total, index out of range) 'No device available'\n",
            );
        }
        for _ in 0..health::OPEN_THRESHOLD {
            r.store(PLAYBACK);
            r.feed(
                &mut m,
                &format!("[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Error: Loading failed\n"),
            );
        }
        let label = format!("{no_devices} {state:?}");
        assert_eq!(r.gate_open(), blamed, "{label}");
        assert_eq!(
            r.hint.as_ref().is_some_and(|h| h.should_prefer_og(SOURCE)),
            blamed,
            "{label}"
        );
        assert_eq!(r.cached(PLAYBACK), !blamed, "{label}");
        let probes = if no_devices {
            1 + health::OPEN_THRESHOLD
        } else {
            0
        };
        assert_eq!(r.probes.load(Ordering::SeqCst), probes, "{label}");
        let text = r.out.logs();
        if !blamed {
            assert!(text.contains("VRChat has no sound output: "), "{text}");
            assert!(
                text.contains("video failed to load because VRChat has no sound output: "),
                "{text}"
            );
            assert!(text.contains("(not a resolver fault)"));
            assert_eq!(
                r.sink.all(),
                <[(std::string::String, &str, i32, std::option::Option<i32>); 0]>::default()
            );
            assert!(r.file_log().contains(&format!(
                "load_failure_no_sound_output ms=0 state={}",
                state.name()
            )));
        }
    }
}

#[tokio::test]
async fn the_driver_tails_a_real_log_and_fires_the_stall_timer() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let log = logs.write("output_log_a.txt", "boot\n");
    let mut deps = r.deps(AudioCheck::new(AudioState::Ok, None));
    deps.silent_stall_window = ms(300);
    let monitor = r.monitor_with(deps);
    let cancel = CancellationToken::new();
    let task = monitor.spawn(vrcr_watchdog::clock::system(), cancel.clone());
    tokio::time::sleep(ms(200)).await;
    logs.append(&log, &format!("{}\n", opening(PLAYBACK)));
    let deadline = tokio::time::Instant::now() + secs(5);
    while r.sink.all().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(ms(25)).await;
    }
    let all = r.sink.all();
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].0.as_str(), all[0].1),
        (PLAYBACK, feedback::SILENT_STALL)
    );
    assert!((300..1500).contains(&all[0].2), "{}", all[0].2);
    cancel.cancel();
    assert!(tokio::time::timeout(secs(2), task).await.is_ok());
}

#[test]
fn session_line_sums_the_whole_run() {
    let a = Activity {
        resolves_total: 12,
        resolves_via_lh_yt: 2,
        resolves_cache_hits: 3,
        reconnects: 1,
        relay_requests: 4000,
        relay_bytes: 900 * 1024 * 1024,
        upstream_relay_bytes: 512 * 1024 * 1024,
        ..Activity::default()
    };
    let usage = ProcessUsage {
        cpu_100ns: 1_234_000_000,
        working_set: 50 * 1024 * 1024,
        peak_working_set: 150 * 1024 * 1024,
        private_bytes: 45 * 1024 * 1024,
        handles: 320,
    };
    assert_eq!(
        perf::session_line(secs(2 * 3600 + 13 * 60), &a, Some(&usage)),
        "[session] up=2h13m resolves=12 cached=3 local_video=2 relay_req=4000 relay_mib=900.0 upstream_mib=512.0 reconnects=1 cpu_s=123.4 peak_ws_mib=150.0 ws_mib=50.0 private_mib=45.0 handles=320"
    );
    assert_eq!(
        perf::session_line(secs(30), &Activity::default(), None),
        "[session] up=0m resolves=0 cached=0 local_video=0 relay_req=0 relay_mib=0.0 upstream_mib=0.0 reconnects=0"
    );
}
