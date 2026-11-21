use std::time::Duration;

use crate::common::{ManualClock, T_2026_09_29};
use vrcr_shared::b64;
use vrcr_watchdog::emitted::{ENTRY_TTL, EmittedUrls, MAX_ENTRIES, MAX_IDS, Origin};

const SOURCE: &str = "https://www.youtube.com/watch?v=abcdefghijk";
const PROXY: &str = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

fn origin(source: &str, cid: Option<&str>) -> Origin {
    Origin {
        source_url: source.to_owned(),
        correlation_id: cid.map(str::to_owned),
    }
}

fn gateway(target: &str) -> String {
    format!(
        "https://localhost.youtube.com:65396/play/abc/manifest.m3u8?target={}",
        b64::encode(target)
    )
}

#[test]
fn emitted_urls_map_back_to_their_source_and_request() {
    let map = EmittedUrls::default();
    map.remember(PROXY, &origin(SOURCE, Some("req-1")));
    assert_eq!(
        map.origin_by_url(PROXY),
        Some(origin(SOURCE, Some("req-1")))
    );
    assert_eq!(map.origin_by_url(" https://other.example/x.m3u8 "), None);
    assert_eq!(map.origin_by_url(""), None);
}

#[test]
fn trust_gateway_urls_resolve_through_their_target() {
    let map = EmittedUrls::default();
    map.remember(PROXY, &origin(SOURCE, None));
    assert_eq!(
        map.origin_by_url(&gateway(PROXY)).map(|o| o.source_url),
        Some(SOURCE.to_owned())
    );
}

#[test]
fn lookups_fall_back_to_the_normalized_url() {
    let map = EmittedUrls::default();
    map.remember(
        "https://live.cdn4.io/ttfcvr/index.m3u8",
        &origin("https://live.cdn4.io/ttfcvr/index.m3u8", None),
    );
    assert!(
        map.origin_by_url("HTTPS://Live.CDN4.io/ttfcvr/index.m3u8#x")
            .is_some()
    );
}

#[test]
fn request_ids_live_in_their_own_map() {
    let map = EmittedUrls::default();
    map.remember(PROXY, &origin(SOURCE, Some("cid-42")));
    map.remember_id("srv-42", &origin(SOURCE, Some("cid-42")));
    assert_eq!(map.len(), 1);
    assert_eq!(map.id_count(), 1);
    assert_eq!(map.origin_by_url("srv-42"), None);
    assert_eq!(
        map.origin_by_id("srv-42").and_then(|o| o.correlation_id),
        Some("cid-42".to_owned())
    );
    assert_eq!(
        map.origin_by_id("cid-42").map(|o| o.source_url),
        Some(SOURCE.to_owned())
    );
    assert_eq!(map.origin_by_id(PROXY), None);
    assert_eq!(map.origin_by_id(" "), None);
}

#[test]
fn the_id_map_stays_bounded_and_keeps_the_newest_ids() {
    let map = EmittedUrls::default();
    for i in 0..(MAX_IDS + 10) {
        map.remember_id(
            &format!("req-{i}"),
            &origin(&format!("https://example.com/v{i}"), None),
        );
    }
    assert_eq!(map.id_count(), MAX_IDS);
    assert_eq!(map.len(), 0);
    assert_eq!(map.origin_by_id("req-0"), None);
    assert_eq!(
        map.origin_by_id(&format!("req-{}", MAX_IDS + 9))
            .map(|o| o.source_url),
        Some(format!("https://example.com/v{}", MAX_IDS + 9))
    );
}

#[test]
fn a_revocation_with_an_unknown_request_id_falls_back_to_its_correlation_id() {
    let map = EmittedUrls::default();
    map.remember_id("req-1", &origin(SOURCE, Some("cid-1")));
    let found = map
        .origin_for_revocation("srv-unknown", Some("cid-1"), None)
        .unwrap_or_else(|| panic!("matched by correlation id"));
    assert_eq!(found.source_url, SOURCE);
    let hint = vrcr_watchdog::og_hint::OgFallbackHint::default();
    let (_, lease) = vrcr_watchdog::og_hint::arm(None, Some(&hint), &found.source_url);
    assert!(lease.is_some());
    assert!(hint.should_prefer_og(SOURCE));
}

#[test]
fn a_revocation_with_unknown_ids_falls_back_to_its_url() {
    let map = EmittedUrls::default();
    map.remember(PROXY, &origin(SOURCE, Some("req-1")));
    let found = map.origin_for_revocation("srv-unknown", Some("cid-unknown"), Some(PROXY));
    assert_eq!(found.map(|o| o.source_url), Some(SOURCE.to_owned()));
    let raw = map.origin_for_revocation(
        "srv-unknown",
        Some("cid-unknown"),
        Some("https://live.cdn4.io/ttfcvr/index.m3u8"),
    );
    assert_eq!(
        raw,
        Some(origin(
            "https://live.cdn4.io/ttfcvr/index.m3u8",
            Some("cid-unknown")
        ))
    );
    assert_eq!(map.origin_for_revocation("srv-unknown", None, None), None);
}

#[test]
fn a_known_request_id_wins_over_the_revocation_url() {
    let map = EmittedUrls::default();
    map.remember_id("req-1", &origin(SOURCE, None));
    let found = map.origin_for_revocation("req-1", None, Some("https://other.example/x.m3u8"));
    assert_eq!(found.map(|o| o.source_url), Some(SOURCE.to_owned()));
}

#[test]
fn remembering_the_same_url_again_replaces_its_origin() {
    let map = EmittedUrls::default();
    map.remember(PROXY, &origin(SOURCE, Some("old")));
    map.remember(PROXY, &origin(SOURCE, Some("new")));
    assert_eq!(map.len(), 1);
    assert_eq!(
        map.origin_by_url(PROXY).and_then(|o| o.correlation_id),
        Some("new".to_owned())
    );
}

#[test]
fn blank_keys_and_sources_are_ignored() {
    let map = EmittedUrls::default();
    map.remember("  ", &origin(SOURCE, None));
    map.remember(PROXY, &origin(" ", None));
    assert!(map.is_empty());
}

#[test]
fn entries_expire_after_their_ttl() {
    let clock = ManualClock::utc(T_2026_09_29);
    let map = EmittedUrls::new(clock.now());
    map.remember(PROXY, &origin(SOURCE, None));
    clock.advance(ENTRY_TTL);
    assert!(map.origin_by_url(PROXY).is_some());
    clock.advance(Duration::from_secs(1));
    assert_eq!(map.origin_by_url(PROXY), None);
}

#[test]
fn the_map_keeps_only_the_newest_entries() {
    let map = EmittedUrls::default();
    for i in 0..(MAX_ENTRIES + 10) {
        map.remember(
            &format!("https://cdn.example.com/{i}.m3u8"),
            &origin(&format!("https://example.com/v{i}"), None),
        );
    }
    assert_eq!(map.len(), MAX_ENTRIES);
    assert_eq!(map.origin_by_url("https://cdn.example.com/0.m3u8"), None);
    assert_eq!(
        map.origin_by_url(&format!("https://cdn.example.com/{}.m3u8", MAX_ENTRIES + 9))
            .map(|o| o.source_url),
        Some(format!("https://example.com/v{}", MAX_ENTRIES + 9))
    );
}
