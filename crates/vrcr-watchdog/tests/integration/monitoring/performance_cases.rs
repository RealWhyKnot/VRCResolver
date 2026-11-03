use super::*;

#[test]
fn series_reports_nearest_rank_percentiles() {
    let mut s = Series::default();
    assert_eq!(s.spread(), None);
    for v in (1..=20).rev() {
        s.push(v * 10);
    }
    s.push(-5);
    assert_eq!(
        s.spread(),
        Some(Spread {
            count: 21,
            p50: 100,
            p95: 190,
            max: 200
        })
    );

    let mut one = Series::default();
    one.push(42);
    assert_eq!(
        one.spread(),
        Some(Spread {
            count: 1,
            p50: 42,
            p95: 42,
            max: 42
        })
    );
}

#[test]
fn series_caps_samples_but_keeps_count_and_max() {
    let mut s = Series::default();
    for _ in 0..PERF_SAMPLE_CAP {
        s.push(5);
    }
    s.push(9_000);
    s.push(i64::MAX);
    let spread = s.spread().unwrap_or_else(|| panic!("no spread"));
    assert_eq!(
        spread.count,
        u64::try_from(PERF_SAMPLE_CAP).expect("sample cap fits u64") + 2
    );
    assert_eq!((spread.p50, spread.p95), (5, 5));
    assert_eq!(spread.max, u32::MAX);
}

#[test]
fn perf_window_records_nothing_until_enabled() {
    let stats = Stats::new(SystemTime::now());
    stats
        .perf
        .record_relay(RelayKind::Segment, false, 10, 20, 100);
    stats
        .perf
        .record_resolve(ResolveStatus::Resolved, false, Duration::from_millis(900));
    stats.perf.record_mesh_rtt(Duration::from_millis(30));
    assert!(!stats.perf.enabled());
    assert_eq!(stats.perf.take(), PerfSnapshot::default());
}

#[test]
fn perf_window_sorts_samples_and_resets_on_take() {
    let stats = Stats::new(SystemTime::now());
    stats.perf.enable();
    stats
        .perf
        .record_relay(RelayKind::Segment, false, 40, 120, 1000);
    stats
        .perf
        .record_relay(RelayKind::Manifest, false, 30, 60, 200);
    stats.perf.record_relay(RelayKind::Other, false, 5, 5, 50);
    stats
        .perf
        .record_relay(RelayKind::Segment, true, 0, 3000, 0);
    let ms = Duration::from_millis;
    stats
        .perf
        .record_resolve(ResolveStatus::Resolved, false, ms(1800));
    stats
        .perf
        .record_resolve(ResolveStatus::Resolved, true, ms(1));
    stats
        .perf
        .record_resolve(ResolveStatus::Fallback, false, ms(9000));
    stats
        .perf
        .record_resolve(ResolveStatus::Failed, false, ms(15000));
    stats
        .perf
        .record_resolve(ResolveStatus::Unexpected, false, ms(10));
    stats.perf.record_mesh_rtt(ms(35));

    let mut snap = stats.perf.take();
    assert!(snap.is_active());
    assert_eq!(
        (snap.relay_requests, snap.relay_errors, snap.relay_bytes),
        (4, 1, 1250)
    );
    assert_eq!(
        snap.segment_header.spread().map(|s| (s.count, s.max)),
        Some((1, 40))
    );
    assert_eq!(
        snap.segment_total.spread().map(|s| (s.count, s.max)),
        Some((1, 120))
    );
    assert_eq!(
        snap.manifest_total.spread().map(|s| (s.count, s.max)),
        Some((1, 60))
    );
    assert_eq!(
        (
            snap.resolves,
            snap.resolves_cached,
            snap.resolves_fallback,
            snap.resolves_failed
        ),
        (5, 1, 1, 2)
    );
    assert_eq!(
        snap.resolve_ms.spread().map(|s| (s.count, s.max)),
        Some((1, 1800))
    );
    assert_eq!(snap.mesh_rtt.spread().map(|s| s.max), Some(35));
    assert_eq!(stats.perf.take(), PerfSnapshot::default());

    stats.perf.record_mesh_rtt(ms(35));
    assert!(!stats.perf.take().is_active());
}

#[test]
fn perf_line_drops_empty_sections() {
    let stats = Stats::new(SystemTime::now());
    stats.perf.enable();
    stats
        .perf
        .record_relay(RelayKind::Segment, false, 40, 120, 3 * 1024 * 1024);
    stats
        .perf
        .record_relay(RelayKind::Segment, false, 60, 200, 1024 * 1024);
    stats.perf.record_mesh_rtt(Duration::from_millis(35));
    let mut snap = stats.perf.take();
    let usage = ProcessUsage {
        cpu_100ns: 0,
        working_set: 48 * 1024 * 1024,
        peak_working_set: 64 * 1024 * 1024,
        private_bytes: 40 * 1024 * 1024 + 512 * 1024,
        handles: 312,
    };
    assert_eq!(
        perf::perf_line(secs(60), &mut snap, Some(&usage), Some(1.26)),
        "[perf] window=60s relay_req=2 relay_err=0 relay_mib=4.0 seg_header_ms=p50:40,p95:60,max:60,n:2 seg_total_ms=p50:120,p95:200,max:200,n:2 mesh_rtt_ms=p50:35,p95:35,max:35,n:1 cpu=1.3% ws_mib=48.0 private_mib=40.5 handles=312"
    );

    stats
        .perf
        .record_resolve(ResolveStatus::Resolved, false, Duration::from_millis(2100));
    stats
        .perf
        .record_resolve(ResolveStatus::Resolved, true, Duration::from_millis(1));
    let mut snap = stats.perf.take();
    assert_eq!(
        perf::perf_line(secs(23), &mut snap, None, None),
        "[perf] window=23s resolves=2 cached=1 resolve_ms=p50:2100,p95:2100,max:2100,n:1"
    );
}

#[test]
fn cpu_percent_is_a_share_of_every_core() {
    let wall = secs(10);
    assert_eq!(perf::cpu_percent(0, 10_000_000, wall, 1), Some(10.0));
    assert_eq!(perf::cpu_percent(0, 10_000_000, wall, 4), Some(2.5));
    assert_eq!(perf::cpu_percent(500, 100, wall, 4), Some(0.0));
    assert_eq!(perf::cpu_percent(0, 100, Duration::ZERO, 4), None);
    assert_eq!(perf::cpu_percent(0, 10_000_000, wall, 0), Some(10.0));
}
