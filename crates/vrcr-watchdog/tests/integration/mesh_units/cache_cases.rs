use super::*;

#[test]
fn welcome_cache_missing_file_is_a_miss() {
    let c = cache_dir();
    assert_eq!(cache(&c).get("us1.vrcresolver.com"), None);
}

#[test]
fn welcome_cache_round_trips_per_node() {
    let c = cache_dir();
    let first = WelcomeFrame {
        engines: owned(&["yt-dlp"]),
        features: owned(&["v3_compression"]),
        warp_active: Some(true),
        server_version: Some("v1".to_owned()),
        yt_dlp_version: Some("yd1".to_owned()),
        ..welcome(Some("node1"))
    };
    let second = WelcomeFrame {
        engines: owned(&["yt-dlp", "ffmpeg"]),
        features: owned(&["v3_compression", "welcome_hash_ack"]),
        warp_active: Some(false),
        server_version: Some("v2".to_owned()),
        yt_dlp_version: Some("yd2".to_owned()),
        ..welcome(Some("node2"))
    };
    let writer = cache(&c);
    writer.store("us1.vrcresolver.com", &first, "hash1");
    writer.store("eu1.vrcresolver.com", &second, "hash2");
    let reopened = cache(&c);
    let a = reopened
        .get("us1.vrcresolver.com")
        .unwrap_or_else(|| panic!("us1 missing"));
    let b = reopened
        .get("eu1.vrcresolver.com")
        .unwrap_or_else(|| panic!("eu1 missing"));
    assert_eq!(a.welcome_hash.as_deref(), Some("hash1"));
    assert_eq!(a.node.as_deref(), Some("node1"));
    assert_eq!(a.warp_active, Some(true));
    assert_eq!(a.server_version.as_deref(), Some("v1"));
    assert_eq!(a.yt_dlp_version.as_deref(), Some("yd1"));
    assert!(a.stored_at.is_some());
    assert_eq!(b.welcome_hash.as_deref(), Some("hash2"));
    assert_eq!(b.engines.as_ref().map(Vec::len), Some(2));
    assert_eq!(b.warp_active, Some(false));
}

#[test]
fn welcome_cache_nodes_do_not_evict_each_other() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("us1.vrcresolver.com", &welcome(Some("node1")), "h1");
    wc.store("eu1.vrcresolver.com", &welcome(Some("node2")), "h2");
    assert_eq!(
        wc.get("us1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h1")
    );
    assert_eq!(
        wc.get("eu1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h2")
    );
}

#[test]
fn welcome_cache_lookup_ignores_ascii_case_and_restore_replaces() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("US1.vrcresolver.com", &welcome(None), "h1");
    assert_eq!(
        wc.get("us1.VRCRESOLVER.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h1")
    );
    wc.store("us1.vrcresolver.com", &welcome(None), "h2");
    let text = fs::read_to_string(&c.path).unwrap_or_default();
    assert!(!text.contains("US1."), "{text}");
    assert_eq!(
        wc.get("US1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h2")
    );
}

#[test]
fn welcome_cache_invalidate_removes_only_that_node() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("us1.vrcresolver.com", &welcome(None), "h1");
    wc.store("eu1.vrcresolver.com", &welcome(None), "h2");
    wc.invalidate("us1.vrcresolver.com");
    assert_eq!(wc.get("us1.vrcresolver.com"), None);
    assert!(wc.get("eu1.vrcresolver.com").is_some());
}

#[test]
fn welcome_cache_corrupt_file_is_a_miss() {
    let c = cache_dir();
    fs::write(&c.path, "{ this is not valid json").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cache(&c).get("us1.vrcresolver.com"), None);
}

#[test]
fn welcome_cache_null_file_is_a_miss_and_can_be_overwritten() {
    let c = cache_dir();
    fs::write(&c.path, "null").unwrap_or_else(|e| panic!("{e}"));
    let wc = cache(&c);
    assert_eq!(wc.get("n"), None);
    wc.store("n", &welcome(None), "h");
    assert!(wc.get("n").is_some());
}

#[test]
fn welcome_cache_write_leaves_no_temp_file() {
    let c = cache_dir();
    cache(&c).store("node1", &welcome(None), "h");
    assert!(c.path.exists());
    assert!(!identity::sibling(&c.path, ".new").exists());
}

#[test]
fn welcome_cache_empty_node_is_a_miss() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("", &welcome(None), "h");
    assert_eq!(wc.get(""), None);
    assert!(!c.path.exists());
}

#[test]
fn welcome_cache_empty_hash_is_not_stored() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("node1", &welcome(None), "");
    assert_eq!(wc.get("node1"), None);
    assert!(!c.path.exists());
}

#[test]
fn welcome_cache_oversized_file_is_a_miss_and_moved_aside() {
    let c = cache_dir();
    let cap = usize::try_from(welcome_cache::MAX_FILE_BYTES).unwrap_or(65_536);
    let mut bytes = vec![0u8; cap + 1];
    bytes[0] = b'{';
    fs::write(&c.path, &bytes).unwrap_or_else(|e| panic!("{e}"));
    let log_path = c.dir.join("watchdog.log");
    let wc = WelcomeCache::new(c.path.clone(), Log::to_file(&log_path));
    assert_eq!(wc.get("us1.vrcresolver.com"), None);
    assert!(!c.path.exists());
    let aside: Vec<String> = list(&c.dir)
        .into_iter()
        .filter(|n| n.starts_with("v3_welcome_cache.json.oversized-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(
        fs::metadata(c.dir.join(&aside[0]))
            .map(|m| m.len())
            .unwrap_or(0),
        welcome_cache::MAX_FILE_BYTES + 1
    );
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    assert!(log.contains("[v3-cache] oversized cache file"), "{log}");
}

#[cfg(windows)]
#[test]
fn welcome_cache_failed_save_cleans_its_temp_file() {
    let c = cache_dir();
    let wc = cache(&c);
    wc.store("us1.vrcresolver.com", &welcome(None), "h0");
    {
        let _lock = lock_exclusively(&c.path);
        wc.store("us1.vrcresolver.com", &welcome(None), "h1");
    }
    assert!(!identity::sibling(&c.path, ".new").exists());
    assert_eq!(
        wc.get("us1.vrcresolver.com")
            .and_then(|e| e.welcome_hash)
            .as_deref(),
        Some("h0")
    );
}

#[test]
fn welcome_cache_keeps_the_welcome_hosts_fields() {
    let c = cache_dir();
    let wc = cache(&c);
    let frame = WelcomeFrame {
        features: owned(&[feature::WELCOME_HOSTS]),
        first_party_hosts: owned(&["vrcresolver.com", "whyknot.dev"]),
        playback_proxy_paths: owned(&["/api/proxy", "/api/stream"]),
        ..welcome(Some("node1"))
    };
    wc.store("us1.vrcresolver.com", &frame, "hash1");
    let entry = wc
        .get("us1.vrcresolver.com")
        .unwrap_or_else(|| panic!("entry missing"));
    assert_eq!(
        entry.first_party_hosts,
        owned(&["vrcresolver.com", "whyknot.dev"])
    );
    assert_eq!(
        entry.playback_proxy_paths,
        owned(&["/api/proxy", "/api/stream"])
    );
}
