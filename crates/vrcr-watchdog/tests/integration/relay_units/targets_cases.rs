use super::*;

#[test]
fn encode_target_param_round_trips() {
    let url = "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=AbC-_dEf=&extra=hi%20there";
    let encoded = encode_target_param(url);
    for bad in ['+', '/', '=', ' '] {
        assert!(!encoded.contains(bad));
    }
    assert_eq!(b64::decode(&encoded).as_deref(), Some(url));
}

#[test]
fn local_prefix_is_the_directory_of_the_play_path() {
    for (path, expected) in [
        ("/play/7f8a9b0c1d2e/manifest.m3u8", "/play/7f8a9b0c1d2e/"),
        (
            "/play/7f8a9b0c1d2e/sub/manifest.m3u8",
            "/play/7f8a9b0c1d2e/sub/",
        ),
        ("/play/manifest.m3u8", "/play/"),
        ("/play", "/play/"),
        ("/play/", "/play/"),
        ("", "/play/"),
        ("/", "/play/"),
        ("/playlist/manifest.m3u8", "/play/"),
        ("/playback/manifest.m3u8", "/play/"),
    ] {
        assert_eq!(local_prefix_for_path(path), expected, "{path}");
    }
    assert!(is_play_path("/PLAY/x"));
    assert!(!is_play_path("/playlist"));
}

#[test]
fn relative_target_maps_namespaced_segment_to_upstream_base() {
    let base = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/";
    assert_eq!(
        resolve_relative_target(
            "/play/7f8a9b0c1d2e/seg_000001.ts",
            "?range=bytes%3D0-",
            "/play/7f8a9b0c1d2e/",
            base
        )
        .as_deref(),
        Some(
            "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?range=bytes%3D0-"
        )
    );
    let manifest =
        Url::parse("https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8?token=abc")
            .unwrap_or_else(|e| panic!("{e}"));
    let dir = manifest
        .join(".")
        .unwrap_or_else(|e| panic!("{e}"))
        .to_string();
    assert_eq!(
        resolve_relative_target(
            "/play/7f8a9b0c1d2e/seg_000002.ts",
            "",
            "/play/7f8a9b0c1d2e/",
            &dir
        )
        .as_deref(),
        Some("https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000002.ts")
    );
    assert_eq!(
        resolve_relative_target("/play/other/seg_000001.ts", "", "/play/7f8a9b0c1d2e/", base),
        None
    );
    assert_eq!(
        resolve_relative_target("/play/7f8a9b0c1d2e/", "", "/play/7f8a9b0c1d2e/", base),
        None
    );
}

#[test]
fn target_param_registers_a_relative_namespace() {
    let resolver = TargetResolver::new();
    let manifest_url = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8";
    let param = encode_target_param(manifest_url);
    let first = resolver
        .resolve(
            &policy(),
            "/play/7f8a9b0c1d2e/manifest.m3u8",
            &format!("?target={param}"),
            Some(&param),
        )
        .unwrap_or_else(|| panic!("manifest target"));
    assert_eq!(first.kind, TargetKind::Target);
    assert_eq!(first.url, manifest_url);
    let seg = resolver
        .resolve(&policy(), "/play/7F8A9B0C1D2E/seg_000003.ts", "", None)
        .unwrap_or_else(|| panic!("segment target"));
    assert_eq!(seg.kind, TargetKind::Relative);
    assert_eq!(
        seg.url,
        "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000003.ts"
    );
    assert!(
        resolver
            .resolve(&policy(), "/play/unknown/seg.ts", "", None)
            .is_none()
    );
}

#[test]
fn disallowed_or_garbage_targets_resolve_but_never_register_a_base() {
    let resolver = TargetResolver::new();
    let evil = encode_target_param("https://example.com/api/proxy/index.m3u8");
    let t = resolver
        .resolve(&policy(), "/play/a/manifest.m3u8", "", Some(&evil))
        .unwrap_or_else(|| panic!("target"));
    assert_eq!(t.kind, TargetKind::Target);
    assert_eq!(resolver.remembered(), 0);
    let junk = resolver
        .resolve(&policy(), "/play/a/manifest.m3u8", "", Some("!!!"))
        .unwrap_or_else(|| panic!("target"));
    assert_eq!(junk.url, "");
    assert!(
        resolver
            .resolve(&policy(), "/play/a/seg.ts", "", None)
            .is_none()
    );
}

#[test]
fn longest_registered_prefix_wins() {
    let resolver = TargetResolver::new();
    let outer = encode_target_param("https://us1.vrcresolver.com/api/proxy/outer/index.m3u8");
    let inner =
        encode_target_param("https://us1.vrcresolver.com/api/proxy/inner/deeper/index.m3u8");
    resolver.resolve(&policy(), "/play/a/manifest.m3u8", "", Some(&outer));
    resolver.resolve(&policy(), "/play/a/sub/manifest.m3u8", "", Some(&inner));
    let t = resolver
        .resolve(&policy(), "/play/a/sub/seg_1.ts", "", None)
        .unwrap_or_else(|| panic!("target"));
    assert_eq!(
        t.url,
        "https://us1.vrcresolver.com/api/proxy/inner/deeper/seg_1.ts"
    );
    let t = resolver
        .resolve(&policy(), "/play/a/seg_2.ts", "", None)
        .unwrap_or_else(|| panic!("target"));
    assert_eq!(
        t.url,
        "https://us1.vrcresolver.com/api/proxy/outer/seg_2.ts"
    );
}

#[test]
fn only_first_party_proxy_targets_are_allowed() {
    for (target, expected) in [
        (
            "https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            true,
        ),
        (
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            true,
        ),
        (
            "https://vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8",
            true,
        ),
        (
            "https://vrcresolver.com/api/stream/0123456789abcdef01234567.m3u8?rsedge=node1",
            true,
        ),
        (
            "https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc",
            true,
        ),
        (
            "https://whyknot.dev/api/proxy/lazy-hls/wk_abc/index.m3u8",
            true,
        ),
        (
            "https://us1.vrcresolver.com/api/restream/shared-status",
            false,
        ),
        ("https://example.com/api/proxy/manifest.m3u8?q=abc", false),
        ("file:///C:/Windows/win.ini", false),
        ("not a url", false),
    ] {
        assert_eq!(
            check_target_url(&policy(), target).is_ok(),
            expected,
            "{target}"
        );
    }
    assert_eq!(
        check_target_url(&policy(), "nope"),
        Err("target_not_absolute")
    );
    assert_eq!(
        check_target_url(&policy(), "ftp://vrcresolver.com/api/proxy/x"),
        Err("target_scheme_not_http")
    );
    assert_eq!(
        check_target_url(&policy(), "https://example.com/api/proxy/x"),
        Err("target_not_first_party_playback_proxy")
    );
}

#[test]
fn server_provided_families_extend_the_target_policy() {
    let p = FirstPartyPolicy::with_server_provided(Some(&["example.net"]), Some(&["/api/media"]));
    assert!(check_target_url(&p, "https://cdn.example.net/api/media/seg.ts").is_ok());
    assert!(check_target_url(&policy(), "https://cdn.example.net/api/media/seg.ts").is_err());
}

#[test]
fn query_value_is_case_insensitive_and_percent_decoded() {
    assert_eq!(
        query_value("a=1&TARGET=x%2Dy", "target").as_deref(),
        Some("x-y")
    );
    assert_eq!(query_value("target=", "target").as_deref(), Some(""));
    assert_eq!(query_value("other=1", "target"), None);
}

#[test]
fn short_url_redacts_invalid_input() {
    assert_eq!(short_url(""), "?");
    let long = "x".repeat(120);
    assert_eq!(short_url(&long), "?");
    assert_eq!(short_url(&"y".repeat(100)), "?");
}
