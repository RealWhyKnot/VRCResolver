mod common;

use std::fs;
use std::time::{Duration, SystemTime};

use common::ManualClock;
use tempfile::TempDir;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::{self, iso8601_utc, parse_iso8601_utc};
use vrcr_shared::wire::{ResolveResponse, action};
use vrcr_watchdog::resolve_cache::{Key, MAX_FILE_BYTES, ResolveCache};
use vrcr_watchdog::stats::Stats;

const NODE: &str = "us1.vrcresolver.com";
const YT: &str = "https://www.youtube.com/watch?v=x";

struct Fixture {
    dir: TempDir,
    clock: ManualClock,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            dir: TempDir::new().unwrap_or_else(|e| panic!("{e}")),
            clock: ManualClock::at(SystemTime::now()),
        }
    }

    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("resolve_cache.json")
    }

    fn cache(&self) -> ResolveCache {
        ResolveCache::new(self.path(), self.clock.now(), Log::disabled())
    }

    fn in_hours(&self, h: u64) -> String {
        iso8601_utc(self.clock.get() + Duration::from_secs(h * 3600))
    }
}

fn key<'a>(node: &'a str, url: &'a str, player: &'a str, format: Option<&'a str>, height: i32) -> Key<'a> {
    Key {
        node,
        url,
        player: Some(player),
        format_arg: format,
        max_height: Some(height),
    }
}

fn resolved(expires_at: Option<String>) -> ResolveResponse {
    ResolveResponse {
        action: action::RESOLVED.to_owned(),
        id: "ignored-on-store".to_owned(),
        url: Some(format!(
            "https://stream.example.com/{}",
            uuid::Uuid::new_v4().simple()
        )),
        engine: Some("yt-dlp:no-cookies-default".to_owned()),
        container: Some("mp4".to_owned()),
        video_codec: Some("h264".to_owned()),
        audio_codec: Some("aac".to_owned()),
        protocol: Some("https".to_owned()),
        audio_channels: Some(2),
        expires_at,
        ..ResolveResponse::default()
    }
}

fn playback(url: &str, expires_at: String) -> ResolveResponse {
    ResolveResponse {
        action: action::RESOLVED.to_owned(),
        id: "ignored-on-store".to_owned(),
        url: Some(url.to_owned()),
        engine: Some("yt-dlp:no-cookies-default".to_owned()),
        protocol: Some("hls".to_owned()),
        expires_at: Some(expires_at),
        ..ResolveResponse::default()
    }
}

#[test]
fn lookup_on_missing_file_returns_none() {
    let f = Fixture::new();
    assert!(
        f.cache()
            .lookup(&key(NODE, YT, "avpro", None, 1080), "req-1")
            .is_none()
    );
}

#[test]
fn store_then_lookup_round_trips_with_id_restamped() {
    let f = Fixture::new();
    let c = f.cache();
    let resp = resolved(Some(f.in_hours(2)));
    let k = key(NODE, YT, "avpro", Some("(mp4/best)[height<=?1080]"), 1080);
    c.store(&k, &resp);

    let hit = c.lookup(&k, "req-2").unwrap_or_else(|| panic!("miss"));
    assert_eq!(hit.action, action::RESOLVED);
    let json = String::from_utf8_lossy(&hit.frame).into_owned();
    assert!(json.contains("\"id\":\"req-2\""), "{json}");
    assert!(!json.contains("ignored-on-store"));
    assert!(json.contains(resp.url.as_deref().unwrap_or("?")));
}

#[test]
fn lookups_that_differ_in_any_key_part_miss() {
    let f = Fixture::new();
    let c = f.cache();
    c.store(
        &key(NODE, YT, "avpro", Some("fmt-A"), 1080),
        &resolved(Some(f.in_hours(1))),
    );

    assert!(
        c.lookup(&key(NODE, YT, "unity", Some("fmt-A"), 1080), "r")
            .is_none()
    );
    assert!(
        c.lookup(&key(NODE, YT, "avpro", Some("fmt-B"), 1080), "r")
            .is_none()
    );
    assert!(
        c.lookup(&key("eu1.vrcresolver.com", YT, "avpro", Some("fmt-A"), 1080), "r")
            .is_none()
    );
    assert!(
        c.lookup(&key(NODE, YT, "avpro", Some("fmt-A"), 2160), "r")
            .is_none()
    );
    assert!(
        c.lookup(&key(NODE, YT, "avpro", Some("fmt-A"), 1080), "r")
            .is_some()
    );
}

#[test]
fn store_skips_fallback_native_responses() {
    let f = Fixture::new();
    let c = f.cache();
    let resp = ResolveResponse {
        action: action::FALLBACK_NATIVE.to_owned(),
        reason: Some("discovery_in_progress".to_owned()),
        expires_at: Some(f.in_hours(1)),
        ..ResolveResponse::default()
    };
    assert!(c.store(&key(NODE, YT, "avpro", None, 1080), &resp).is_none());
    assert_eq!(c.count(), 0);
}

#[test]
fn store_applies_the_default_ttl_when_the_server_omits_expires_at() {
    let f = Fixture::new();
    let c = f.cache();
    let k = key(NODE, YT, "avpro", None, 1080);
    let effective = c.store(&k, &resolved(None)).unwrap_or_default();
    let at = parse_iso8601_utc(&effective).unwrap_or_else(|| panic!("{effective}"));
    assert_eq!(
        at.duration_since(f.clock.get()).ok(),
        Some(Duration::from_secs(300))
    );
    assert_eq!(c.count(), 1);
    assert!(c.lookup(&k, "r").is_some());
}

#[test]
fn lookup_treats_an_entry_inside_the_safety_margin_as_a_miss_and_evicts_it() {
    let f = Fixture::new();
    let c = f.cache();
    let k = key(NODE, YT, "avpro", None, 1080);
    c.store(
        &k,
        &resolved(Some(iso8601_utc(f.clock.get() + Duration::from_secs(5)))),
    );
    assert_eq!(c.count(), 1);
    assert!(c.lookup(&k, "r").is_none());
    assert_eq!(c.count(), 0);
}

#[test]
fn entries_expire_as_the_clock_moves() {
    let f = Fixture::new();
    let c = f.cache();
    let k = key(NODE, YT, "avpro", None, 1080);
    c.store(&k, &resolved(Some(f.in_hours(1))));
    f.clock.advance(Duration::from_secs(3600 - 31));
    assert!(c.lookup(&k, "r").is_some());
    f.clock.advance(Duration::from_secs(2));
    assert!(c.lookup(&k, "r").is_none());
}

#[test]
fn evict_by_url_drops_every_combo_for_that_url() {
    let f = Fixture::new();
    let c = f.cache();
    let url = "https://www.youtube.com/watch?v=stale";
    let other = "https://other.example.com/x";
    c.store(
        &key(NODE, url, "avpro", Some("fmt-A"), 1080),
        &resolved(Some(f.in_hours(1))),
    );
    c.store(
        &key(NODE, url, "avpro", Some("fmt-B"), 1080),
        &resolved(Some(f.in_hours(1))),
    );
    c.store(
        &key(NODE, url, "unity", None, 1080),
        &resolved(Some(f.in_hours(1))),
    );
    c.store(
        &key(NODE, other, "avpro", None, 1080),
        &resolved(Some(f.in_hours(1))),
    );
    assert_eq!(c.count(), 4);

    assert_eq!(c.evict_by_url(url), 3);
    assert_eq!(c.count(), 1);
    assert!(
        c.lookup(&key(NODE, url, "avpro", Some("fmt-A"), 1080), "r")
            .is_none()
    );
    assert!(c.lookup(&key(NODE, other, "avpro", None, 1080), "r").is_some());
}

#[test]
fn evict_by_url_matches_the_resolved_playback_url() {
    let f = Fixture::new();
    let c = f.cache();
    let source = "https://www.youtube.com/watch?v=stale";
    let play = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc";
    let k = key(NODE, source, "avpro", None, 1080);
    c.store(&k, &playback(play, f.in_hours(1)));
    assert_eq!(c.evict_by_url(play), 1);
    assert!(c.lookup(&k, "r").is_none());
}

#[test]
fn source_url_for_resolved_round_trips_after_store() {
    let f = Fixture::new();
    let c = f.cache();
    let source = "https://www.youtube.com/watch?v=abc";
    let play = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=xyz";
    c.store(
        &key(NODE, source, "avpro", None, 1080),
        &playback(play, f.in_hours(1)),
    );
    assert_eq!(c.source_url_for_resolved(play).as_deref(), Some(source));
    assert!(
        c.source_url_for_resolved("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=missing")
            .is_none()
    );
}

#[test]
fn cap_evicts_the_oldest_fetched_at_first() {
    let f = Fixture::new();
    let c = f.cache();
    let expires = f.in_hours(10);
    for i in 0..502 {
        let url = format!("https://example.com/v={i}");
        c.store(
            &key(NODE, &url, "avpro", None, 1080),
            &resolved(Some(expires.clone())),
        );
        f.clock.advance(Duration::from_millis(1));
    }
    assert_eq!(c.count(), 500);
    assert!(
        c.lookup(&key(NODE, "https://example.com/v=0", "avpro", None, 1080), "r")
            .is_none()
    );
    assert!(
        c.lookup(&key(NODE, "https://example.com/v=1", "avpro", None, 1080), "r")
            .is_none()
    );
    assert!(
        c.lookup(&key(NODE, "https://example.com/v=501", "avpro", None, 1080), "r")
            .is_some()
    );
}

#[test]
fn persisted_state_survives_flush_and_a_fresh_instance() {
    let f = Fixture::new();
    let k = key(
        NODE,
        "https://www.youtube.com/watch?v=persist",
        "avpro",
        Some("fmt"),
        1080,
    );
    let first = f.cache();
    first.store(&k, &resolved(Some(f.in_hours(2))));
    first.flush_now();
    assert!(f.path().exists());

    let hit = f
        .cache()
        .lookup(&k, "r-after-restart")
        .unwrap_or_else(|| panic!("miss"));
    assert!(String::from_utf8_lossy(&hit.frame).contains("\"id\":\"r-after-restart\""));
}

#[test]
fn debounced_flush_writes_the_file_without_an_explicit_flush() {
    let f = Fixture::new();
    let c = ResolveCache::with_debounce(
        f.path(),
        f.clock.now(),
        Log::disabled(),
        Duration::from_millis(50),
    );
    c.store(
        &key(NODE, YT, "avpro", None, 1080),
        &resolved(Some(f.in_hours(1))),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !f.path().exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = fs::read_to_string(f.path()).unwrap_or_default();
    assert!(text.starts_with("{\"version\":1,\"entries\":{"), "{text}");
    assert!(!f.dir.path().join("resolve_cache.json.new").exists());
}

#[test]
fn file_format_matches_the_old_client() {
    let f = Fixture::new();
    let c = f.cache();
    let k = key(NODE, YT, "avpro", Some("fmt"), 1080);
    let expires = f.in_hours(1);
    c.store(
        &k,
        &ResolveResponse {
            action: action::RESOLVED.to_owned(),
            id: "i".to_owned(),
            url: Some("https://x/".to_owned()),
            expires_at: Some(expires.clone()),
            ..ResolveResponse::default()
        },
    );
    c.flush_now();
    let v: serde_json::Value =
        serde_json::from_slice(&fs::read(f.path()).unwrap_or_default()).unwrap_or_default();
    let hash = k.hash();
    let e = &v["entries"][&hash];
    assert_eq!(e["key"], hash.as_str());
    assert_eq!(e["node"], NODE);
    assert_eq!(e["url"], YT);
    assert_eq!(e["player"], "avpro");
    assert_eq!(e["format_arg"], "fmt");
    assert_eq!(e["expires_at"], expires.as_str());
    assert_eq!(e["response"]["url"], "https://x/");
    assert_eq!(e["fetched_at"], iso8601_utc(f.clock.get()).as_str());
}

#[test]
fn key_hash_matches_the_old_client_layout() {
    let k = key(NODE, YT, "avpro", Some("fmt"), 1080);
    let raw = format!("{NODE}\u{1f}{YT}\u{1f}avpro\u{1f}1080\u{1f}fmt");
    use sha2::Digest;
    let digest = sha2::Sha256::digest(raw.as_bytes());
    assert_eq!(k.hash(), hex::encode(&digest[..16]));
    assert_eq!(k.hash().len(), 32);
    let none = Key {
        node: NODE,
        url: YT,
        player: None,
        format_arg: None,
        max_height: None,
    };
    let digest = sha2::Sha256::digest(format!("{NODE}\u{1f}{YT}\u{1f}\u{1f}\u{1f}").as_bytes());
    assert_eq!(none.hash(), hex::encode(&digest[..16]));
}

#[test]
fn load_drops_already_expired_entries() {
    let f = Fixture::new();
    let fresh = f.in_hours(2);
    let stale = iso8601_utc(f.clock.get() + Duration::from_secs(5));
    let entry = |k: &str, exp: &str| {
        format!(
            "\"{k}\":{{\"key\":\"{k}\",\"node\":\"n\",\"url\":\"u-{k}\",\"player\":\"avpro\",\"format_arg\":\"f\",\"response\":{{\"action\":\"resolved\",\"id\":\"i\",\"url\":\"https://x/\",\"expires_at\":\"{exp}\"}},\"fetched_at\":\"2026-01-01T00:00:00Z\",\"expires_at\":\"{exp}\"}}"
        )
    };
    let text = format!(
        "{{\"version\":1,\"entries\":{{{},{}}}}}",
        entry("keep", &fresh),
        entry("drop", &stale)
    );
    fs::write(f.path(), text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(f.cache().count(), 1);
}

#[test]
fn corrupt_file_loads_as_an_empty_cache() {
    let f = Fixture::new();
    fs::write(f.path(), "this is not json{[}").unwrap_or_else(|e| panic!("{e}"));
    let c = f.cache();
    assert_eq!(c.count(), 0);
    let k = key(NODE, YT, "avpro", None, 1080);
    c.store(&k, &resolved(Some(f.in_hours(1))));
    assert!(c.lookup(&k, "r").is_some());
}

#[test]
fn oversized_file_is_renamed_aside_and_treated_as_a_miss() {
    let f = Fixture::new();
    fs::write(f.path(), vec![0u8; MAX_FILE_BYTES as usize + 1]).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(f.cache().count(), 0);
    assert!(!f.path().exists());
    let aside: Vec<_> = fs::read_dir(f.dir.path())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("resolve_cache.json.oversized-")
        })
        .collect();
    assert_eq!(aside.len(), 1);
}

#[test]
fn iso8601_parser_reads_the_shapes_servers_and_the_old_client_write() {
    let base = parse_iso8601_utc("2026-10-14T05:06:07Z").unwrap_or_else(|| panic!("basic"));
    assert_eq!(iso8601_utc(base), "2026-10-14T05:06:07.0000000Z");
    for (text, offset_ms) in [
        ("2026-10-14T05:06:07.1234567Z", 123),
        ("2026-10-14T05:06:07.5+00:00", 500),
        ("2026-10-14T07:06:07+02:00", 0),
        ("2026-10-14T00:06:07-05:00", 0),
        ("2026-10-14T05:06:07", 0),
        (" 2026-10-14 05:06:07z ", 0),
    ] {
        let t = parse_iso8601_utc(text).unwrap_or_else(|| panic!("{text}"));
        let delta = t.duration_since(base).unwrap_or_default().as_millis();
        assert_eq!(delta, offset_ms, "{text}");
    }
    for bad in [
        "",
        "2026-10-14",
        "2026-13-01T00:00:00Z",
        "2026-10-14T05:06:07.Z",
        "2026-10-14T05:06:07+0200",
        "garbage",
    ] {
        assert!(parse_iso8601_utc(bad).is_none(), "{bad}");
    }
    let now = SystemTime::now();
    let back = parse_iso8601_utc(&logutil::iso8601_utc(now)).unwrap_or_else(|| panic!("round trip"));
    assert!(now.duration_since(back).unwrap_or_default() < Duration::from_micros(1));
}

#[test]
fn stats_count_upstream_relay_traffic_separately() {
    let s = Stats::default();
    let now = SystemTime::now();
    s.record_relay_request(true);
    s.record_relay_bytes(true, 100, now);
    s.record_relay_bytes(false, 25, now);
    let a = s.activity();
    assert_eq!(a.relay_requests, 1);
    assert_eq!(a.relay_bytes, 125);
    assert_eq!(a.upstream_relay_requests, 1);
    assert_eq!(a.upstream_relay_bytes, 100);
    assert!(a.last_relay.is_some() && a.last_upstream_relay.is_some());
    assert!(a.relay_active(now, Duration::from_secs(1)));
    assert!(a.upstream_active(now, Duration::from_secs(1)));
    assert!(!a.relay_active(now + Duration::from_secs(2), Duration::from_secs(1)));
}

#[test]
fn stats_bandwidth_reports_recent_bytes_by_second() {
    let s = Stats::default();
    let now = parse_iso8601_utc("2026-05-10T12:00:10Z").unwrap_or_else(|| panic!("time"));
    s.record_relay_bytes(true, 1000, now - Duration::from_secs(2));
    s.record_relay_bytes(true, 2000, now);
    s.record_relay_bytes(true, 3000, now);
    let b = s.bandwidth(now, 4);
    assert_eq!(b.history, [0, 1000, 0, 5000]);
    assert_eq!(b.current, 5000);
    assert_eq!(b.peak, 5000);
    assert!(b.has_traffic());
}
