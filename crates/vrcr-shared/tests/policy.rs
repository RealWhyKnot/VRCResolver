#![allow(clippy::unwrap_used)]

use std::time::Duration;

use vrcr_shared::budget::{self, retry};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::trust_gateway;

const NONE: Option<&[&str]> = None;

fn builtin() -> FirstPartyPolicy {
    FirstPartyPolicy::default()
}

#[test]
fn playback_proxy_urls_accept_only_first_party_proxy_shapes() {
    let cases = [
        ("https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true),
        ("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true),
        ("https://proxy.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true),
        (
            "https://proxy.whyknot.dev/api/proxy/seg.ts?url=abc&wkedge=node1",
            true,
        ),
        ("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true),
        ("https://whyknot.dev/api/proxy/lazy-hls/wk_abc/index.m3u8", true),
        (
            "https://node2.whyknot.dev/api/popcorn/proxy/manifest.m3u8?clientId=c&index=1",
            true,
        ),
        (
            "https://node2.whyknot.dev/api/popcorn/proxy/seg.ts?clientId=c&url=abc",
            true,
        ),
        ("https://node1.whyknot.dev/api/restream/shared-status", false),
        ("https://example.com/api/proxy/manifest.m3u8?q=abc", false),
        ("https://notwhyknot.dev/api/proxy/manifest.m3u8?q=abc", false),
        ("https://evil-whyknot.dev/api/proxy/manifest.m3u8?q=abc", false),
        (
            "https://evil-vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            false,
        ),
        (
            "https://vrcresolver.com.evil.com/api/proxy/manifest.m3u8?q=abc",
            false,
        ),
        ("not a url", false),
    ];
    let p = builtin();
    for (url, expected) in cases {
        assert_eq!(p.is_playback_proxy_url(url), expected, "{url}");
    }
}

#[test]
fn first_party_hosts_cover_both_families() {
    let cases = [
        ("vrcresolver.com", true),
        ("us1.vrcresolver.com", true),
        ("whyknot.dev", true),
        ("node1.whyknot.dev", true),
        ("evil-whyknot.dev", false),
        ("vrcresolver.com.evil.com", false),
        ("example.com", false),
    ];
    let p = builtin();
    for (host, expected) in cases {
        assert_eq!(p.is_first_party_host(host), expected, "{host}");
    }
}

#[test]
fn trust_gateway_extension_keeps_the_avpro_dispatch_extension() {
    let cases = [
        ("https://proxy.whyknot.dev/api/proxy/manifest.m3u8?q=abc", "m3u8"),
        ("https://node1.whyknot.dev/api/proxy/manifest.mpd?q=abc", "mpd"),
        ("https://node1.whyknot.dev/api/proxy?q=legacy", "m3u8"),
        (
            "https://node1.whyknot.dev/api/popcorn/proxy/manifest.m3u8?clientId=c&index=1",
            "m3u8",
        ),
        ("https://cdn.example.com/video/seg.ts?sig=abc", "ts"),
        ("https://node1.whyknot.dev/api/proxy?url=legacy", "bin"),
        ("https://cdn.example.com/video/no-extension?sig=abc", ""),
        ("not a url", ""),
    ];
    let p = builtin();
    for (url, expected) in cases {
        assert_eq!(p.trust_gateway_extension(url), expected, "{url}");
    }
}

#[test]
fn trust_gateway_wraps_only_first_party_playback_targets() {
    let p = builtin();
    let target = "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc";
    let local = trust_gateway::build(&p, 51234, target, Some("test-session"), "http").unwrap();
    assert!(local.starts_with("http://localhost.youtube.com:51234/play/testsession/manifest.m3u8?target="));
    assert_eq!(trust_gateway::extract_target(&local).as_deref(), Some(target));
    assert!(
        trust_gateway::build(
            &p,
            51234,
            "https://cdn.example.com/video/seg.ts",
            Some("s"),
            "http"
        )
        .is_none()
    );
    assert!(
        trust_gateway::build(
            &p,
            51234,
            "http://localhost.youtube.com:51234/play/a/manifest.m3u8?target=x",
            Some("s"),
            "http"
        )
        .is_none()
    );
    assert!(
        trust_gateway::build(
            &p,
            51234,
            "https://node1.whyknot.dev/api/proxy/manifest.m3u8?note=localhost.youtube.com",
            Some("s"),
            "http"
        )
        .is_some()
    );
}

#[test]
fn trust_gateway_can_emit_https_urls() {
    let p = builtin();
    let target = "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc";
    let local = trust_gateway::build(&p, 51234, target, Some("test-session"), "https").unwrap();
    assert!(local.starts_with("https://localhost.youtube.com:51234/play/testsession/manifest.m3u8?target="));
    assert_eq!(trust_gateway::extract_target(&local).as_deref(), Some(target));
}

#[test]
fn trust_gateway_rejects_bad_ports_and_generates_a_session_when_none_is_given() {
    let p = builtin();
    let target = "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc";
    assert!(trust_gateway::build(&p, 1024, target, None, "http").is_none());
    assert!(trust_gateway::build(&p, 65536, target, None, "http").is_none());
    assert!(trust_gateway::build(&p, 51234, target, Some("!!!"), "http").is_none());
    let local = trust_gateway::build(&p, 51234, target, None, "http").unwrap();
    let session = local.split('/').nth(4).unwrap();
    assert_eq!(session.len(), 12);
    let long = "a".repeat(100);
    let local = trust_gateway::build(&p, 51234, target, Some(&long), "http").unwrap();
    assert_eq!(local.split('/').nth(4).unwrap().len(), 64);
}

#[test]
fn trust_gateway_allows_only_http_and_https_schemes() {
    for (scheme, expected) in [
        ("http", true),
        ("https", true),
        ("HTTPS", true),
        ("ftp", false),
        ("", false),
    ] {
        assert_eq!(trust_gateway::is_allowed_scheme(scheme), expected, "{scheme}");
    }
}

#[test]
fn trust_gateway_local_host_list_is_exact() {
    let cases = [
        ("localhost.youtube.com", true),
        ("LOCALHOST.YOUTUBE.COM", true),
        ("localhost", true),
        ("127.0.0.1", true),
        ("notlocalhost.youtube.com", false),
        ("localhost.youtube.com.evil.com", false),
        ("youtube.com", false),
    ];
    for (host, expected) in cases {
        assert_eq!(trust_gateway::is_local_gateway_host(host), expected, "{host}");
    }
}

#[test]
fn trust_gateway_extract_rejects_non_gateway_urls() {
    assert!(
        trust_gateway::extract_target("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc").is_none()
    );
    assert!(
        trust_gateway::extract_target(
            "http://localhost.youtube.com:51234/play/a/manifest.m3u8?target=not-base64"
        )
        .is_none()
    );
}

#[test]
fn wrap_for_relay_passes_through_without_a_live_relay() {
    let dir = tempfile::tempdir().unwrap();
    let p = builtin();
    let target = "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc";
    assert_eq!(
        trust_gateway::wrap_for_relay(&p, dir.path(), target, |_| true),
        target
    );
    std::fs::write(dir.path().join("relay_port.txt"), "51234\n").unwrap();
    assert_eq!(
        trust_gateway::wrap_for_relay(&p, dir.path(), target, |_| false),
        target
    );
    let wrapped = trust_gateway::wrap_for_relay(&p, dir.path(), target, |port| port == 51234);
    assert!(wrapped.starts_with("http://localhost.youtube.com:51234/play/"));
    std::fs::write(dir.path().join("relay_scheme.txt"), "HTTPS").unwrap();
    let wrapped = trust_gateway::wrap_for_relay(&p, dir.path(), target, |_| true);
    assert!(wrapped.starts_with("https://localhost.youtube.com:51234/play/"));
    let other = "https://cdn.example.com/v.mp4";
    assert_eq!(
        trust_gateway::wrap_for_relay(&p, dir.path(), other, |_| true),
        other
    );
}

#[test]
fn server_provided_hosts_extend_the_built_ins_and_never_replace_them() {
    let p = FirstPartyPolicy::with_server_provided(Some(&["example-cdn.net"]), NONE);
    assert!(p.is_first_party_host("example-cdn.net"));
    assert!(p.is_first_party_host("node1.example-cdn.net"));
    assert!(p.is_first_party_host("vrcresolver.com"));
    assert!(p.is_first_party_host("proxy.whyknot.dev"));
    assert!(!p.is_first_party_host("evil-example-cdn.net.attacker.io"));
}

#[test]
fn server_provided_hosts_drop_invalid_entries() {
    for entry in [
        "127.0.0.1",
        "::1",
        "localhost",
        "https://example.com",
        "bad host.com",
        "",
    ] {
        let p = FirstPartyPolicy::with_server_provided(Some(&[entry]), NONE);
        assert!(!p.is_first_party_host(entry), "{entry}");
        assert!(p.host_families().is_empty(), "{entry}");
    }
}

#[test]
fn server_provided_hosts_cap_at_eight_entries() {
    let many: Vec<String> = (0..12).map(|i| format!("host{i}.example")).collect();
    let p = FirstPartyPolicy::with_server_provided(Some(&many), NONE);
    assert!(p.is_first_party_host("host7.example"));
    assert!(!p.is_first_party_host("host8.example"));
}

#[test]
fn server_provided_paths_must_stay_under_api() {
    let paths = [
        "/api/next-proxy",
        "/anything",
        "/api/",
        "/api/x/../y",
        "/api/trail/",
    ];
    let p = FirstPartyPolicy::with_server_provided(NONE, Some(&paths));
    assert!(p.is_playback_proxy_url("https://vrcresolver.com/api/next-proxy/manifest.m3u8"));
    assert!(!p.is_playback_proxy_url("https://vrcresolver.com/anything/manifest.m3u8"));
    assert!(!builtin().is_playback_proxy_url("https://vrcresolver.com/api/next-proxy/x.m3u8"));
}

#[test]
fn re_ask_is_redundant_only_after_a_hintless_validation_reject() {
    assert!(budget::re_ask_is_redundant(
        Some("validation_failed_http_403"),
        None
    ));
    assert!(budget::re_ask_is_redundant(
        Some("validation_failed_range_fetch_error"),
        None
    ));
    assert!(!budget::re_ask_is_redundant(
        Some("validation_failed_http_403"),
        Some(5000)
    ));
    assert!(!budget::re_ask_is_redundant(Some("discovery_in_progress"), None));
    assert!(!budget::re_ask_is_redundant(Some("all_configs_failed"), None));
    assert!(!budget::re_ask_is_redundant(None, None));
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn og_window_follows_the_budget_rules() {
    assert_eq!(budget::og_window(ms(26_500), true, None), budget::OG_RESERVE);
    assert_eq!(budget::og_window(ms(100), true, None), ms(22_900));
    assert_eq!(budget::og_window(ms(27_000), true, None), budget::OG_RESERVE);
    assert_eq!(budget::og_window(ms(600_000), true, None), budget::OG_RESERVE);
    assert_eq!(budget::og_window(ms(26_000), false, None), budget::OG_HARD_CAP);
    assert_eq!(budget::og_window(ms(16_000), true, Some(41_500)), ms(12_000));
    assert_eq!(budget::og_window(ms(16_000), true, Some(20_000)), ms(7_000));
    assert_eq!(
        budget::og_window(ms(26_500), true, Some(41_500)),
        budget::OG_RESERVE
    );
    assert_eq!(
        budget::og_window(ms(26_000), false, Some(41_500)),
        budget::OG_HARD_CAP
    );
}

#[test]
fn og_window_after_a_fast_decline_leaves_the_re_ask_above_its_minimum() {
    let elapsed = ms(100);
    let window = budget::og_window(elapsed, true, None);
    let left = budget::TOTAL - elapsed - window;
    assert!(i64::try_from(left.as_millis()).unwrap() >= retry::MIN_BUDGET_FOR_RETRY_MS);
}

#[test]
fn re_ask_delay_follows_the_server_hint() {
    assert_eq!(budget::re_ask_delay_ms(20_000, None), Some(0));
    assert_eq!(budget::re_ask_delay_ms(25_500, None), None);
    assert_eq!(budget::re_ask_delay_ms(17_000, Some(8_000)), Some(0));
    assert_eq!(budget::re_ask_delay_ms(4_500, Some(5_000)), Some(500));
    assert_eq!(budget::re_ask_delay_ms(16_500, Some(41_500)), None);
    assert_eq!(budget::re_ask_delay_ms(100, Some(25_750)), Some(25_650));
    assert_eq!(budget::re_ask_delay_ms(26_500, Some(8_000)), None);
}
