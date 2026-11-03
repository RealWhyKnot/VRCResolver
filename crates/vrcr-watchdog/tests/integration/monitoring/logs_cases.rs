use super::*;

#[test]
fn offset_rescan_and_resolution_tables() {
    assert_eq!(initial_read_offset_for_new_file(1234, true), 1234);
    assert_eq!(initial_read_offset_for_new_file(1234, false), 0);
    for (has, since, expected) in [
        (false, 0, true),
        (false, 100, true),
        (true, 1000, false),
        (true, 4999, false),
        (true, 5000, true),
        (true, -1, true),
    ] {
        assert_eq!(
            should_rescan_directory(has, since),
            expected,
            "{has} {since}"
        );
    }
    let cases: &[(&str, Option<(i32, i32)>)] = &[
        (
            "[Always] [Video Playback] Switched to 1920x1080",
            Some((1920, 1080)),
        ),
        (
            "[AVProVideo] PostStateChanged: OpeningToPlaying fwidth=1280 fheight=720",
            Some((1280, 720)),
        ),
        (
            "[Video Playback] switched to resolution 640x360",
            Some((640, 360)),
        ),
        ("Switched to 8x8", None),
        ("Switched to 1920x9999", None),
        ("Switched to 99999x1080", None),
        ("fwidth=1280 only", None),
        ("fheight=720 fwidth=1280", None),
        ("   ", None),
        ("Unswitched to 1920x1080", None),
    ];
    for (line, expected) in cases {
        assert_eq!(parse_observed_resolution(line), *expected, "{line}");
    }
}

#[test]
fn first_log_is_tailed_from_its_end_and_truncation_restarts() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut m = r.monitor();
    let log = logs.write("output_log_a.txt", &format!("{}\n", opening(PLAYBACK)));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), None);
    let b = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b";
    fs::write(&log, format!("{}\n", opening(b))).unwrap_or_else(|e| panic!("{e}"));
    logs.touch(&log);
    r.clock.advance(secs(1));
    m.poll_once(r.now());
    assert_eq!(
        m.last_opened_url(),
        None,
        "a shrunk file resets without replaying"
    );
    fs::write(&log, "x\n").unwrap_or_else(|e| panic!("{e}"));
    logs.touch(&log);
    r.clock.advance(secs(1));
    m.poll_once(r.now());
    logs.append(&log, &format!("{}\n", opening(b)));
    r.clock.advance(secs(1));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(b));
}

#[test]
fn half_written_line_stays_with_its_own_log() {
    let r = Rig::new();
    let mut logs = LogDir::new(r.dir.path());
    let mut m = r.monitor();
    let (a1, b1) = (
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=a1",
        "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=b1",
    );
    let a = logs.write(
        "output_log_a.txt",
        "2026.10.11 10:00:00 Debug      -  boot\n",
    );
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.append(
        &a,
        "2026.10.11 10:00:05 Debug      -  [AVProVideo] Opening https://us1.vrcresolver.com/api/proxy/mani",
    );
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.write("output_log_b.txt", &format!("{}\n", opening(b1)));
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(b1));
    logs.append(&a, "fest.m3u8?q=a1 (offset 0) with API MediaFoundation\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(a1));
}

#[test]
fn missing_directory_and_non_log_files_are_ignored() {
    let r = Rig::new();
    let mut m = r.monitor();
    m.poll_once(r.now());
    let mut logs = LogDir::new(r.dir.path());
    logs.write("debug.txt", &format!("{}\n", opening(PLAYBACK)));
    logs.write("output_log_x.log", &format!("{}\n", opening(PLAYBACK)));
    fs::create_dir_all(logs.path.join("output_log_dir.txt")).unwrap_or_else(|e| panic!("{e}"));
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    let log = logs.write("OUTPUT_LOG_upper.TXT", "boot\n");
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    logs.append(&log, &format!("{}\n", opening(PLAYBACK)));
    r.clock.advance(secs(10));
    m.poll_once(r.now());
    assert_eq!(m.last_opened_url(), Some(PLAYBACK));
}
