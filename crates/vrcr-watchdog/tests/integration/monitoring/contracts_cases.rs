use super::*;

#[test]
fn resolution_is_stored_for_the_current_url_and_confirms() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    r.feed(&mut m, &format!("[AVProVideo] Opening {PLAYBACK}\n"));
    r.clock.advance(ms(1500));
    r.feed(&mut m, "[Always] [Video Playback] Switched to 1920x1080\n");
    assert_eq!(m.observed_delivered_height(PLAYBACK, r.now()), Some(1080));
    assert_eq!(
        r.sink.all(),
        vec![(PLAYBACK.to_owned(), feedback::PLAYING, 1500, Some(1080))]
    );
}

#[test]
fn partial_lines_wait_for_the_rest() {
    let r = Rig::new();
    let mut m = r.monitor();
    let first = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=first";
    let second = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=second";
    r.feed(
        &mut m,
        &format!(
            "{}\n2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani",
            opening(first)
        ),
    );
    assert_eq!(m.last_opened_url(), Some(first));
    r.feed(
        &mut m,
        "fest.m3u8?q=second (offset 0) with API MediaFoundation\r\n",
    );
    assert_eq!(m.last_opened_url(), Some(second));
}

#[test]
fn a_multibyte_character_split_across_reads_survives() {
    let r = Rig::new();
    let mut m = r.monitor();
    let line = format!("[AVProVideo] Opening {PLAYBACK}&t=\u{00e9}\n");
    let bytes = line.as_bytes();
    let cut = bytes.len() - 2;
    m.process_bytes(&bytes[..cut], r.now());
    assert_eq!(m.last_opened_url(), None);
    m.process_bytes(&bytes[cut..], r.now());
    assert_eq!(
        m.last_opened_url(),
        Some(format!("{PLAYBACK}&t=\u{00e9}").as_str())
    );
}

#[test]
fn poll_waits_for_a_line_the_game_is_still_writing() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut m = r.monitor();
    let url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a";
    let log = logs.write(
        "output_log_a.txt",
        "2026.10.11 10:00:00 Debug      -  boot\n",
    );
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.append(
        &log,
        "2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani",
    );
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), None);
    logs.append(&log, "fest.m3u8?q=a (offset 0) with API MediaFoundation\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(url));
}

#[test]
fn repeated_failures_on_our_urls_open_the_gate() {
    let r = Rig::new().with_gate();
    let m = r.monitor();
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a");
    m.mark_playback_failure("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b");
    assert!(!r.gate_open());
    m.mark_playback_failure("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=c");
    assert!(r.gate_open());
    assert!(r.out.logs().contains(
        "resolver paused -- 3 playbacks failed in a row; VRChat's own resolver takes over while things recover"
    ));

    let r = Rig::new().with_gate();
    let m = r.monitor();
    for v in ["a", "b", "c"] {
        m.mark_playback_failure(&format!("https://cdn.example.com/video-{v}.mp4"));
    }
    assert!(!r.gate_open());
}

#[test]
fn accept_line_and_observed_resolution_reset_the_streak() {
    for confirm in [
        "[AVProVideo] Using playback path: MediaFoundation\n",
        "[Always] [Video Playback] Switched to 1920x1080\n",
    ] {
        let r = Rig::new().with_gate();
        let mut m = r.monitor();
        m.mark_playback_failure(PLAYBACK);
        m.mark_playback_failure(PLAYBACK);
        r.feed(
            &mut m,
            &format!("[AVProVideo] Opening {PLAYBACK}\n{confirm}"),
        );
        m.mark_playback_failure(PLAYBACK);
        m.mark_playback_failure(PLAYBACK);
        assert!(!r.gate_open(), "{confirm}");
    }
}

#[test]
fn failure_feedback_carries_the_delivered_height() {
    let r = Rig::new();
    let mut m = r.monitor();
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {PLAYBACK}\nSwitched to 1920x1080\n"),
    );
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Error: Loading failed\n"),
    );
    let all = r.sink.all();
    assert_eq!(
        all.last().map(|f| (f.1, f.3)),
        Some((feedback::LOAD_FAILURE, Some(1080)))
    );
}

#[test]
fn trust_gateway_urls_are_reported_by_their_target() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    let policy = FirstPartyPolicy::default();
    let local =
        trust_gateway::build(&policy, 51234, PLAYBACK, Some("s1"), "http").unwrap_or_default();
    assert!(local.starts_with("http://localhost.youtube.com:51234/play/s1/manifest"));
    assert_eq!(m.canonical(&local), PLAYBACK);
    assert_eq!(
        m.canonical("https://cdn.example.com/a.mp4"),
        "https://cdn.example.com/a.mp4"
    );
    r.feed(
        &mut m,
        &format!("[AVProVideo] Opening {local}\n[AVProVideo] Error: Loading failed\n"),
    );
    assert_eq!(r.sink.all()[0].0, PLAYBACK);
}

#[test]
fn delivered_heights_are_capped() {
    let r = Rig::new();
    let mut m = r.monitor();
    for i in 0..70 {
        r.clock.advance(ms(10));
        r.feed(
            &mut m,
            &format!("[AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/v.m3u8?q={i}\nSwitched to 640x360\n"),
        );
    }
    let now = r.now();
    let kept = (0..70)
        .filter(|i| {
            m.observed_delivered_height(
                &format!("https://us1.vrcresolver.com/api/proxy/v.m3u8?q={i}"),
                now,
            )
            .is_some()
        })
        .count();
    assert_eq!(kept, vrclog::MAX_DELIVERED_HEIGHT_ENTRIES);
    assert!(
        m.observed_delivered_height("https://us1.vrcresolver.com/api/proxy/v.m3u8?q=0", now)
            .is_none()
    );
    assert!(
        m.observed_delivered_height("https://us1.vrcresolver.com/api/proxy/v.m3u8?q=69", now)
            .is_some()
    );
}

#[test]
fn system_line_lists_hardware_and_settings() {
    let info = SystemInfo {
        cpu: Some("AMD Ryzen 9 7950X 16-Core Processor".to_owned()),
        cores: 32,
        memory: Some(MemoryStatus {
            total: 64 * 1024 * 1024 * 1024,
            available: 20 * 1024 * 1024 * 1024 + 300 * 1024 * 1024,
        }),
        disk_free: Some(120 * 1024 * 1024 * 1024),
        settings: r#"{"playback":{"high_quality":false}}"#.to_owned(),
    };
    assert_eq!(
        perf::system_line(&info),
        r#"[startup] system cpu="AMD Ryzen 9 7950X 16-Core Processor" cores=32 ram_gib=64.0 ram_free_gib=20.3 disk_free_gib=120.0 settings={"playback":{"high_quality":false}}"#
    );
    let bare = SystemInfo {
        cores: 1,
        settings: "{}".to_owned(),
        ..SystemInfo::default()
    };
    assert_eq!(
        perf::system_line(&bare),
        "[startup] system cores=1 settings={}"
    );
    assert!(perf::logical_cores() >= 1);
}

#[test]
fn startup_phases_name_each_step_and_the_total() {
    let mut p = Phases::start();
    p.mark("settings");
    std::thread::sleep(Duration::from_millis(5));
    p.mark("relay");
    assert_eq!(p.names(), vec!["settings", "relay"]);
    let line = p.line();
    assert!(line.starts_with("[startup] phases settings="), "{line}");
    assert!(line.contains("ms relay="), "{line}");
    let total: u128 = line
        .rsplit("total=")
        .next()
        .and_then(|t| t.strip_suffix("ms"))
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("{line}"));
    assert!(total >= 5, "{line}");
}

#[cfg(windows)]
#[test]
fn process_usage_reads_this_process() {
    let u = vrcr_shared::win::process_usage().unwrap_or_else(|| panic!("no usage"));
    assert!(u.working_set > 0 && u.peak_working_set >= u.working_set);
    assert!(u.private_bytes > 0 && u.handles > 0);
    let m = vrcr_shared::win::memory_status().unwrap_or_else(|| panic!("no memory status"));
    assert!(m.total > 0 && m.available <= m.total);
    assert!(vrcr_shared::win::cpu_name().is_some_and(|n| !n.is_empty() && !n.contains('\0')));
}
