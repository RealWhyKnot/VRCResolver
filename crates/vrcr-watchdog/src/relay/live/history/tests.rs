use super::*;

#[test]
fn live_history_bounds_expires_refreshes_and_forgets() {
    let history = History::default();
    let now = Instant::now();
    let live = MediaInfo {
        live: true,
        segments: 2880,
        kept: 2880,
        ..MediaInfo::default()
    };
    for index in 0..10_000 {
        history.observe(
            index.to_string(),
            Some(live),
            now + Duration::from_millis(index),
        );
        assert!(history.entries.lock().expect("history").len() <= CAPACITY);
    }
    assert_eq!(history.entries.lock().expect("history").len(), CAPACITY);
    assert!(!history.contains("0", now + Duration::from_secs(10)));
    assert!(history.contains("9999", now + Duration::from_secs(299)));
    assert!(history.contains("9999", now + Duration::from_secs(598)));
    assert!(!history.contains("9999", now + Duration::from_secs(898)));
    history.observe("live".to_owned(), Some(live), now);
    history.observe(
        "live".to_owned(),
        Some(MediaInfo {
            live: false,
            ..live
        }),
        now,
    );
    assert!(!history.contains("live", now));
    history.observe(
        "short".to_owned(),
        Some(MediaInfo {
            segments: 30,
            kept: 30,
            needs_trim: true,
            ..live
        }),
        now,
    );
    assert!(history.contains("short", now));
    assert_eq!(
        identity("https://example.test/live.m3u8?token=one"),
        identity("https://example.test/live.m3u8?token=two")
    );
    assert_ne!(
        identity("https://example.test/live.m3u8"),
        identity("https://example.test/other.m3u8")
    );
    assert_ne!(
        identity("https://example.test/live.m3u8"),
        identity("https://other.test/live.m3u8")
    );
}
