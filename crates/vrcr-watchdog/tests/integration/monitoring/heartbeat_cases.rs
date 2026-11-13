use super::*;

#[test]
fn uptime_and_bytes_formats() {
    for (s, expected) in [
        (0, "0m"),
        (45, "0m"),
        (60, "1m"),
        (59 * 60, "59m"),
        (3600, "1h0m"),
        (2 * 3600 + 13 * 60, "2h13m"),
        (24 * 3600, "1d0h"),
        (50 * 3600 + 30 * 60, "2d2h"),
    ] {
        assert_eq!(heartbeat::format_uptime(secs(s)), expected, "{s}");
    }
    for (b, expected) in [
        (0, "0 B"),
        (1023, "1023 B"),
        (1024, "1.0 KB"),
        (1024 * 1024, "1.0 MB"),
        (1024 * 1024 * 1024, "1.00 GB"),
        (1024_u64.pow(4), "1.00 TB"),
        (1_288_490_188, "1.20 GB"),
    ] {
        assert_eq!(format_bytes(b), expected, "{b}");
    }
    assert_eq!(format_bytes_per_second(2048), "2.0 KB/s");
}

#[test]
fn heartbeat_line_lists_only_what_happened() {
    let quiet = Activity::default();
    assert_eq!(
        heartbeat::line(secs(120), false, &quiet, 0),
        "up=2m mesh=disconnected"
    );
    let busy = Activity {
        resolves_total: 12,
        resolves_via_lh_yt: 3,
        resolves_cache_hits: 4,
        bytes_estimate_total: 1024 * 1024 * 5,
        reconnects: 2,
        ..Activity::default()
    };
    assert_eq!(
        heartbeat::line(secs(3 * 3600 + 5 * 60), true, &busy, 7),
        "up=3h5m mesh=connected resolves=12 (3 local video) video=5.0 MB cache=7(4hits) reconnects=2"
    );
    let hits_only = Activity {
        resolves_cache_hits: 1,
        ..Activity::default()
    };
    assert_eq!(
        heartbeat::line(secs(60), true, &hits_only, 0),
        "up=1m mesh=connected cache=0(1hits)"
    );
}

#[test]
fn heartbeat_stays_quiet_while_the_log_is_busy() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let clock = ManualClock::at(SystemTime::now());
    let out = Capture::default();
    let log = Log::to_file(&dir.path().join("w.log"));
    let stats = Arc::new(Stats::new(clock.get() - secs(90 * 60)));
    stats.record_resolve(true);
    let hb = Heartbeat {
        stats,
        mesh_connected: Box::new(|| true),
        cache_size: Box::new(|| 2),
        now: clock.now(),
        ux: out.ux(),
        log: log.clone(),
    };
    log.line("recent activity");
    assert_eq!(hb.tick(), None);
    clock.advance(heartbeat::QUIET_WINDOW + secs(5));
    assert_eq!(
        hb.tick().as_deref(),
        Some("up=1h35m mesh=connected resolves=1 (1 local video) cache=2(0hits)")
    );
    assert!(out.logs().contains("[heartbeat]"));

    let silent = Heartbeat {
        log: Log::disabled(),
        ..hb
    };
    assert!(silent.tick().is_some());
}
