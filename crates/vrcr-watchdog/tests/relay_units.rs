mod common;

use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::HeaderMap;
use http::header::HeaderValue;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use url::Url;
use vrcr_shared::b64;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_watchdog::relay::activity::play_id;
use vrcr_watchdog::relay::headers::{
    copy_response_headers, forward_request_headers, should_drop_response_header,
};
use vrcr_watchdog::relay::hitch::parse_lazy_segment;
use vrcr_watchdog::relay::manifest::{
    self, MAX_MANIFEST_BYTES, is_likely_manifest, localize, localize_stream,
};
use vrcr_watchdog::relay::security::{check_target_url, is_allowed_host_header};
use vrcr_watchdog::relay::server::query_value;
use vrcr_watchdog::relay::targets::{
    TargetKind, encode_target_param, is_play_path, local_prefix_for_path, resolve_relative_target,
};
use vrcr_watchdog::relay::{
    HitchDetector, RelayActivity, RelayBypass, RelayPortManager, TargetResolver, TimingSample, short_url,
};

use common::mesh::Capture;

fn policy() -> FirstPartyPolicy {
    FirstPartyPolicy::default()
}

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (Ux::new(Box::new(out.clone()), Style::plain(), String::new), out)
}

fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[test]
fn port_manager_replaces_busy_previous_port() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap_or_else(|e| panic!("{e}"));
    let busy_port = busy.local_addr().unwrap_or_else(|e| panic!("{e}")).port();
    fs::write(dir.path().join("relay_last_port.txt"), busy_port.to_string())
        .unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    assert!(ports.initialize());
    assert_ne!(ports.current_port(), busy_port);
    assert_eq!(read(&dir, "relay_port.txt"), ports.current_port().to_string());
    assert_eq!(
        read(&dir, "relay_last_port.txt"),
        ports.current_port().to_string()
    );
    assert!(out.text().contains("(previous port was busy)"), "{}", out.text());
}

#[test]
fn port_manager_reuses_a_free_previous_port() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let free = {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap_or_else(|e| panic!("{e}"));
        l.local_addr().unwrap_or_else(|e| panic!("{e}")).port()
    };
    fs::write(dir.path().join("relay_last_port.txt"), format!(" {free}\n")).unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    assert!(ports.initialize());
    assert_eq!(ports.current_port(), free);
    assert!(
        out.text()
            .contains(&format!("reserved local video port {free} (reused)"))
    );
}

#[test]
fn port_manager_ignores_privileged_or_garbage_previous_ports() {
    for junk in ["80", "1024", "abc", "70000", ""] {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("relay_last_port.txt"), junk).unwrap_or_else(|e| panic!("{e}"));
        let (ux, out) = capture_ux();
        let ports = RelayPortManager::new(dir.path(), ux);
        assert!(ports.initialize());
        assert!(ports.current_port() > 0);
        assert!(!out.text().contains("busy"), "{junk}: {}", out.text());
    }
}

#[test]
fn port_manager_fresh_port_rewrites_relay_files() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    assert!(ports.initialize());
    let first = ports.current_port();
    let _busy = TcpListener::bind((Ipv4Addr::LOCALHOST, first)).unwrap_or_else(|e| panic!("{e}"));
    assert!(ports.try_reserve_fresh_port("test bind failure"));
    assert_ne!(ports.current_port(), 0);
    assert_ne!(ports.current_port(), first);
    assert_eq!(read(&dir, "relay_port.txt"), ports.current_port().to_string());
    assert_eq!(
        read(&dir, "relay_last_port.txt"),
        ports.current_port().to_string()
    );
    assert!(
        out.text()
            .contains(&format!("(previous port {first} failed: test bind failure)"))
    );
    assert!(!dir.path().join("relay_port.txt.tmp").exists());
}

#[test]
fn port_manager_scheme_file_falls_back_to_http() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, _) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    ports.write_scheme_file("HTTPS");
    assert_eq!(read(&dir, "relay_scheme.txt"), "https");
    ports.write_scheme_file("ftp");
    assert_eq!(read(&dir, "relay_scheme.txt"), "http");
    ports.delete_port_file();
    assert!(!dir.path().join("relay_scheme.txt").exists());
}

#[test]
fn play_id_reads_the_second_segment() {
    assert_eq!(play_id("/play/abc123/manifest.m3u8"), Some("abc123"));
    assert_eq!(play_id("/play/abc123/proxy/9f/seg.ts"), Some("abc123"));
    assert_eq!(play_id("/play/abc123"), Some("abc123"));
    for bad in ["", "/play/", "/not-play/abc/manifest.m3u8", "/PLAY/abc/x"] {
        assert_eq!(play_id(bad), None, "{bad}");
    }
}

#[test]
fn wedge_check_ignores_urls_that_are_not_the_local_relay() {
    let a = RelayActivity::new();
    assert_eq!(
        a.wedged_port("https://vrcresolver.com/play/x/manifest.m3u8"),
        None
    );
    assert_eq!(a.wedged_port("https://localhost.youtube.com:5000/other/x"), None);
    assert_eq!(a.wedged_port("not a url"), None);
}

#[test]
fn wedge_check_flags_plays_the_relay_never_saw() {
    let a = RelayActivity::new();
    let url = "https://localhost.youtube.com:54321/play/deadbeef/manifest.m3u8?target=abc";
    assert_eq!(a.wedged_port(url), Some(54321));
    a.record("/play/deadbeef/proxy/1/seg.ts");
    assert_eq!(a.wedged_port(url), None);
    assert_eq!(
        a.wedged_port("http://LOCALHOST.YOUTUBE.COM/play/other/manifest.m3u8"),
        Some(80)
    );
}

#[test]
fn activity_trims_to_half_once_past_the_cap() {
    let a = RelayActivity::new();
    for i in 0..=512 {
        a.record(&format!("/play/p{i}/manifest.m3u8"));
    }
    assert_eq!(a.tracked(), 256);
    assert_eq!(
        a.wedged_port("https://localhost.youtube.com:1/play/p0/x"),
        Some(1)
    );
    assert_eq!(a.wedged_port("https://localhost.youtube.com:1/play/p512/x"), None);
    a.record("/nothing");
    assert_eq!(a.tracked(), 256);
}

#[test]
fn bypass_hides_port_file_while_wedged_and_restores_on_new_session() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = Arc::new(RelayPortManager::new(dir.path(), ux.clone()));
    assert!(ports.initialize());
    ports.write_scheme_file("https");
    let port_file = dir.path().join("relay_port.txt");
    let bypass = RelayBypass::new(ports.clone(), "https", ux);

    assert!(!bypass.engage(ports.current_port() + 1));
    assert!(port_file.exists());
    assert!(bypass.engage(ports.current_port()));
    assert!(bypass.active());
    assert!(!port_file.exists());
    assert!(bypass.engage(ports.current_port()));
    assert_eq!(out.text().matches("can no longer reach").count(), 1);

    bypass.release();
    assert!(!bypass.active());
    assert_eq!(read(&dir, "relay_port.txt"), ports.current_port().to_string());
    assert_eq!(read(&dir, "relay_scheme.txt"), "https");
    assert!(out.text().contains(&format!(
        "new VRChat session; local video relay back on port {}",
        ports.current_port()
    )));
    bypass.release();
    assert_eq!(out.text().matches("new VRChat session").count(), 1);
}

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
        ("/play/7f8a9b0c1d2e/sub/manifest.m3u8", "/play/7f8a9b0c1d2e/sub/"),
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
        Some("https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?range=bytes%3D0-")
    );
    let manifest = Url::parse("https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8?token=abc")
        .unwrap_or_else(|e| panic!("{e}"));
    let dir = manifest.join(".").unwrap_or_else(|e| panic!("{e}")).to_string();
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
    assert!(resolver.resolve(&policy(), "/play/a/seg.ts", "", None).is_none());
}

#[test]
fn longest_registered_prefix_wins() {
    let resolver = TargetResolver::new();
    let outer = encode_target_param("https://us1.vrcresolver.com/api/proxy/outer/index.m3u8");
    let inner = encode_target_param("https://us1.vrcresolver.com/api/proxy/inner/deeper/index.m3u8");
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
    assert_eq!(t.url, "https://us1.vrcresolver.com/api/proxy/outer/seg_2.ts");
}

#[test]
fn relative_bases_stay_bounded() {
    let resolver = TargetResolver::new();
    let target = encode_target_param("https://us1.vrcresolver.com/api/proxy/x/index.m3u8");
    for i in 0..600 {
        resolver.resolve(&policy(), &format!("/play/p{i}/manifest.m3u8"), "", Some(&target));
    }
    assert!(resolver.remembered() <= 512, "{}", resolver.remembered());
    assert!(resolver.remembered() > 256);
}

#[test]
fn host_header_allows_only_expected_local_hosts() {
    for (host, port, expected) in [
        (Some("localhost.youtube.com:51234"), 51234, true),
        (Some("LOCALHOST.YOUTUBE.COM:51234"), 51234, true),
        (Some("127.0.0.1:51234"), 51234, true),
        (Some("localhost:51234"), 51234, true),
        (Some(" localhost.youtube.com "), 51234, true),
        (Some("us1.vrcresolver.com:51234"), 51234, false),
        (Some("localhost.youtube.com:51235"), 51234, false),
        (Some(""), 51234, false),
        (None, 51234, false),
    ] {
        assert_eq!(is_allowed_host_header(host, port), expected, "{host:?}");
    }
}

#[test]
fn only_first_party_proxy_targets_are_allowed() {
    for (target, expected) in [
        ("https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true),
        ("https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc", true),
        (
            "https://vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8",
            true,
        ),
        (
            "https://vrcresolver.com/api/stream/0123456789abcdef01234567.m3u8?rsedge=node1",
            true,
        ),
        ("https://node1.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true),
        ("https://whyknot.dev/api/proxy/lazy-hls/wk_abc/index.m3u8", true),
        ("https://us1.vrcresolver.com/api/restream/shared-status", false),
        ("https://example.com/api/proxy/manifest.m3u8?q=abc", false),
        ("file:///C:/Windows/win.ini", false),
        ("not a url", false),
    ] {
        assert_eq!(check_target_url(&policy(), target).is_ok(), expected, "{target}");
    }
    assert_eq!(check_target_url(&policy(), "nope"), Err("target_not_absolute"));
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
fn response_header_drop_list_matches() {
    for name in [
        "Transfer-Encoding",
        "alt-svc",
        "Server",
        "cf-cache-status",
        "Speculation-Rules",
        "Report-To",
        "NEL",
        "CF-RAY",
        "cf-connecting-ip",
        "Connection",
    ] {
        assert!(should_drop_response_header(name), "{name}");
    }
    for name in ["Content-Range", "Accept-Ranges", "Cache-Control", "ETag", "cfx"] {
        assert!(!should_drop_response_header(name), "{name}");
    }
}

#[test]
fn request_headers_forward_everything_but_credentials_and_framing() {
    let mut src = HeaderMap::new();
    for (k, v) in [
        ("host", "localhost.youtube.com:1"),
        ("cookie", "a=b"),
        ("authorization", "Bearer x"),
        ("accept-encoding", "gzip"),
        ("connection", "keep-alive"),
        ("content-length", "0"),
        ("content-type", "text/plain"),
        ("range", "bytes=10-"),
        ("user-agent", "AVProMobileVideo"),
    ] {
        src.insert(k, HeaderValue::from_static(v));
    }
    let out = forward_request_headers(&src);
    let names: Vec<&str> = out.keys().map(http::HeaderName::as_str).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(out["range"], "bytes=10-");
    assert_eq!(out["user-agent"], "AVProMobileVideo");
    assert_eq!(out["accept-encoding"], "identity");
}

#[test]
fn copied_response_headers_join_values_and_drop_rewritten_encoding() {
    let mut src = HeaderMap::new();
    src.append("set-thing", HeaderValue::from_static("a"));
    src.append("set-thing", HeaderValue::from_static("b"));
    src.insert("content-encoding", HeaderValue::from_static("gzip"));
    src.insert("content-length", HeaderValue::from_static("10"));
    src.insert("content-type", HeaderValue::from_static("video/mp2t"));
    src.insert("cf-ray", HeaderValue::from_static("x"));
    let kept = copy_response_headers(&src, false);
    assert_eq!(kept.headers["set-thing"], "a, b");
    assert_eq!(kept.headers["content-encoding"], "gzip");
    assert!(!kept.headers.contains_key("content-length"));
    assert!(!kept.headers.contains_key("content-type"));
    assert_eq!((kept.passed, kept.dropped), (2, 1));
    let rewritten = copy_response_headers(&src, true);
    assert!(!rewritten.headers.contains_key("content-encoding"));
    assert_eq!((rewritten.passed, rewritten.dropped), (1, 2));
}

#[test]
fn query_value_is_case_insensitive_and_percent_decoded() {
    assert_eq!(query_value("a=1&TARGET=x%2Dy", "target").as_deref(), Some("x-y"));
    assert_eq!(query_value("target=", "target").as_deref(), Some(""));
    assert_eq!(query_value("other=1", "target"), None);
}

#[test]
fn short_url_truncates_at_one_hundred_chars() {
    assert_eq!(short_url(""), "?");
    let long = "x".repeat(120);
    assert_eq!(short_url(&long), format!("{}...", "x".repeat(100)));
    assert_eq!(short_url(&"y".repeat(100)), "y".repeat(100));
}

fn proxy(url: &str, name: &str) -> String {
    let normalized = Url::parse(url).unwrap_or_else(|e| panic!("{e}")).to_string();
    let ns = hex::encode(&Sha256::digest(normalized.as_bytes())[..8]);
    format!("proxy/{ns}/{name}?target={}", b64::encode(&normalized))
}

const SEG1: &str = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=tok";
const SEG2: &str = "https://eu1.vrcresolver.com/api/proxy/seg.ts?a=1&b=2";
const VARIANT: &str = "https://proxy.whyknot.dev/api/proxy/a/index.m3u8?x=1";
const KEY: &str = "https://eu1.vrcresolver.com/api/proxy/key.bin?clientId=c";
const MAP: &str = "https://us1.vrcresolver.com/api/proxy/init~mp4?q=%C3%A9";
const MEDIA: &str = "https://us1.vrcresolver.com/api/proxy/audio/en.m3u8?q=z";

fn golden_input(nl: &str) -> String {
    let lines = [
        "#EXTM3U".to_owned(),
        "#EXT-X-VERSION:6".to_owned(),
        format!("#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"{MEDIA}\""),
        "#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720".to_owned(),
        VARIANT.to_owned(),
        String::new(),
        format!("#EXT-X-KEY:METHOD=AES-128,URI=\"{KEY}\",IV=0x01"),
        format!("#EXT-X-MAP:URI=\"{MAP}\""),
        "#EXTINF:2.000,".to_owned(),
        SEG1.to_owned(),
        "#EXTINF:2.000,".to_owned(),
        format!("   {}  ", SEG2.replace('&', "&amp;")),
        "#EXTINF:2.000,".to_owned(),
        "seg_relative_3.ts".to_owned(),
        "#EXTINF:2.000,".to_owned(),
        "https://cdn.example.com/video/seg.ts?x=1".to_owned(),
        "data:text/plain;base64,AAAA".to_owned(),
        "\t".to_owned(),
        "#EXT-X-ENDLIST".to_owned(),
    ];
    format!("{}{nl}", lines.join(nl))
}

fn golden_expected() -> String {
    [
        "#EXTM3U".to_owned(),
        "#EXT-X-VERSION:6".to_owned(),
        format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"{}\"",
            proxy(MEDIA, "en.m3u8")
        ),
        "#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720".to_owned(),
        proxy(VARIANT, "index.m3u8"),
        String::new(),
        format!(
            "#EXT-X-KEY:METHOD=AES-128,URI=\"{}\",IV=0x01",
            proxy(KEY, "key.bin")
        ),
        format!("#EXT-X-MAP:URI=\"{}\"", proxy(MAP, "proxy.bin")),
        "#EXTINF:2.000,".to_owned(),
        proxy(SEG1, "seg_000001.ts"),
        "#EXTINF:2.000,".to_owned(),
        format!("   {}  ", proxy(SEG2, "seg.ts")),
        "#EXTINF:2.000,".to_owned(),
        "seg_relative_3.ts".to_owned(),
        "#EXTINF:2.000,".to_owned(),
        "https://cdn.example.com/video/seg.ts?x=1".to_owned(),
        "data:text/plain;base64,AAAA".to_owned(),
        "\t".to_owned(),
        "#EXT-X-ENDLIST".to_owned(),
    ]
    .join("\n")
}

fn chunked(input: &[u8], size: usize) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> + Unpin {
    let parts: Vec<std::io::Result<Bytes>> = input
        .chunks(size.max(1))
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    futures_util::stream::iter(parts)
}

fn stream_localize(input: &[u8], size: usize, max: u64) -> (String, manifest::LocalizeResult) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap_or_else(|e| panic!("{e}"));
    rt.block_on(async {
        let mut out: Vec<u8> = Vec::new();
        let r = localize_stream(&policy(), chunked(input, size), &mut out, max)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        (String::from_utf8(out).unwrap_or_else(|e| panic!("{e}")), r)
    })
}

#[test]
fn manifest_golden_output_for_lf_and_crlf_at_every_chunk_size() {
    let expected = golden_expected();
    for nl in ["\n", "\r\n", "\r"] {
        let input = golden_input(nl);
        assert_eq!(localize(&policy(), &input), expected, "{nl:?}");
        for size in [1, 2, 3, 7, 64, 4096, input.len()] {
            let (out, r) = stream_localize(input.as_bytes(), size, MAX_MANIFEST_BYTES);
            assert_eq!(out, expected, "{nl:?} chunk={size}");
            assert!(r.changed);
            assert!(!r.exceeded);
            assert_eq!(r.bytes_out, expected.len() as u64);
        }
    }
}

#[test]
fn manifest_stream_strips_a_utf8_bom_even_when_split() {
    let mut input = b"\xEF\xBB\xBF".to_vec();
    input.extend_from_slice(b"#EXTM3U\nseg1.ts\n");
    for size in [1, 2, 3, 4, 100] {
        let (out, r) = stream_localize(&input, size, MAX_MANIFEST_BYTES);
        assert_eq!(out, "#EXTM3U\nseg1.ts", "chunk={size}");
        assert!(!r.changed);
    }
    let (out, _) = stream_localize(b"\xEF\xBB\xBF", 1, MAX_MANIFEST_BYTES);
    assert_eq!(out, "");
    let (out, _) = stream_localize(b"\xEF\xBB", 1, MAX_MANIFEST_BYTES);
    assert_eq!(out, "\u{FFFD}");
}

#[test]
fn unchanged_manifest_is_reported_unchanged() {
    let (out, r) = stream_localize(b"#EXTM3U\n#EXTINF:2,\nseg1.ts\n\n", 5, MAX_MANIFEST_BYTES);
    assert!(!r.changed);
    assert_eq!(out, "#EXTM3U\n#EXTINF:2,\nseg1.ts\n");
}

#[test]
fn manifest_stream_stops_once_output_exceeds_the_cap() {
    let (out, r) = stream_localize(b"#EXTM3U\n#EXTINF:2,\nseg1.ts\nseg2.ts\n", 4, 12);
    assert!(r.exceeded);
    assert_eq!(out, "#EXTM3U\n#EXTINF:2,");
}

#[test]
fn manifest_rewrites_browser_stream_segments() {
    let upstream = "https://eu1.vrcresolver.com/api/stream/0123456789abcdef01234567/7.ts?rsedge=eu1";
    let out = localize(&policy(), &format!("#EXTM3U\n#EXTINF:4.000,\n{upstream}\n"));
    assert!(!out.contains(upstream));
    assert!(out.contains("proxy/"));
    assert!(out.contains("/7.ts?target="));
}

#[test]
fn manifest_rewrites_proxy_lines_into_targets_the_resolver_accepts() {
    let upstream = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/index.m3u8";
    let out = localize(
        &policy(),
        &format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\n{upstream}\n"),
    );
    assert!(!out.contains("https://us1.vrcresolver.com"));
    let line = out
        .lines()
        .find(|l| l.starts_with("proxy/"))
        .unwrap_or_else(|| panic!("{out}"));
    let (local, query) = line.split_once('?').unwrap_or_else(|| panic!("{line}"));
    assert!(local.ends_with("/index.m3u8"));
    let param = query.strip_prefix("target=").unwrap_or_else(|| panic!("{query}"));
    let t = TargetResolver::new()
        .resolve(
            &policy(),
            &format!("/play/7f8a9b0c1d2e/{local}"),
            &format!("?{query}"),
            Some(param),
        )
        .unwrap_or_else(|| panic!("target"));
    assert_eq!(t.url, upstream);
}

#[test]
fn manifest_rewrites_quoted_uris_and_leaves_relative_lines() {
    let upstream = "https://eu1.vrcresolver.com/api/proxy/seg.ts?url=abc";
    let out = localize(
        &policy(),
        &format!("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"{upstream}\"\n#EXTINF:4.000,\nseg_000001.ts\n"),
    );
    assert!(!out.contains(upstream));
    assert!(out.contains("URI=\"proxy/"));
    assert!(out.contains("/seg.ts?target="));
    assert!(out.contains("\nseg_000001.ts"));
}

#[test]
fn manifest_gives_same_file_name_targets_distinct_namespaces() {
    let out = localize(
        &policy(),
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nhttps://us1.vrcresolver.com/api/proxy/a/index.m3u8?x=1\n#EXT-X-STREAM-INF:BANDWIDTH=2\nhttps://us1.vrcresolver.com/api/proxy/b/index.m3u8?x=2\n",
    );
    let locals: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("proxy/"))
        .map(|l| &l[..l.find("/index.m3u8").unwrap_or(0)])
        .collect();
    assert_eq!(locals.len(), 2);
    assert_ne!(locals[0], locals[1]);
}

#[test]
fn manifest_rewrites_urls_inside_xml_text() {
    let upstream = "https://eu1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.m4s";
    let out = localize(
        &policy(),
        &format!("<MPD><Period><BaseURL>{upstream}</BaseURL></Period></MPD>"),
    );
    assert!(!out.contains(upstream));
    assert!(out.contains("<BaseURL>proxy/"));
    assert!(out.contains("/seg_000001.m4s?target="));
}

#[test]
fn manifest_leaves_third_party_urls_alone() {
    let m = "#EXTM3U\n#EXTINF:4.000,\nhttps://cdn.example.com/video/seg.ts";
    assert_eq!(localize(&policy(), m), m);
}

#[test]
fn manifest_html_decoding_covers_numeric_entities() {
    assert_eq!(manifest::html_decode("a&amp;b&#38;c&#x26;d&lt;"), "a&b&c&d<");
    assert_eq!(manifest::html_decode("a&bogus;b&"), "a&bogus;b&");
    assert_eq!(manifest::html_decode("plain"), "plain");
}

#[test]
fn manifest_streams_a_large_playback_token_manifest() {
    let token = "A".repeat(900);
    let mut text = String::from("#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n");
    for i in 0..5000 {
        text.push_str(&format!(
            "#EXTINF:2.000,\nhttps://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_{i:06}.ts?playback_id={token}\n"
        ));
    }
    text.push_str("#EXT-X-ENDLIST\n");
    assert!(text.len() > 4 * 1024 * 1024);
    let (out, r) = stream_localize(text.as_bytes(), 16 * 1024, MAX_MANIFEST_BYTES);
    assert!(!r.exceeded);
    assert!(r.changed);
    assert!(!out.contains("https://us1.vrcresolver.com"));
    assert_eq!(out.lines().filter(|l| l.starts_with("proxy/")).count(), 5000);
    assert!(out.ends_with("#EXT-X-ENDLIST"));
}

#[test]
fn manifest_stream_and_string_agree() {
    let m = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\nhttps://us1.vrcresolver.com/api/proxy/a/index.m3u8?x=1\n#EXTINF:2,\nhttps://eu1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=tok\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://eu1.vrcresolver.com/api/proxy/key.bin?clientId=c\"\nhttps://cdn.example.com/video/seg.ts\n";
    let (streamed, _) = stream_localize(m.as_bytes(), 9, MAX_MANIFEST_BYTES);
    assert_eq!(streamed, localize(&policy(), m));
}

#[test]
fn manifest_detection_only_matches_manifest_shapes() {
    for (local, target, media, expected) in [
        (
            "/play/a/manifest.m3u8",
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            "text/plain",
            true,
        ),
        (
            "/play/a/index.m3u8",
            "https://us1.vrcresolver.com/api/proxy/index.m3u8",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/manifest.mpd",
            "https://us1.vrcresolver.com/api/proxy/manifest.mpd",
            "application/dash+xml",
            true,
        ),
        (
            "/play/a/manifest.m3u8?token=abc",
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?token=abc",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/manifest.bin",
            "https://us1.vrcresolver.com/api/proxy/manifest.m3u8?q=abc",
            "application/octet-stream",
            true,
        ),
        (
            "/play/a/manifest",
            "https://us1.vrcresolver.com/api/proxy/manifest",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/manifest.mp4",
            "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=abc",
            "video/mp4",
            false,
        ),
        (
            "/play/a/MANIFEST.MP4",
            "https://us1.vrcresolver.com/api/proxy/manifest.mp4?q=abc",
            "video/mp4",
            false,
        ),
        (
            "/play/a/seg.ts",
            "https://us1.vrcresolver.com/api/proxy/seg.ts?url=abc",
            "video/mp2t",
            false,
        ),
        (
            "/play/a/manifest_archive/seg.ts",
            "https://us1.vrcresolver.com/api/proxy/seg.ts",
            "video/mp2t",
            false,
        ),
        (
            "/play/a/foo.ts",
            "https://example.test/playlist.ts",
            "application/vnd.apple.mpegurl",
            true,
        ),
        (
            "/play/a/foo.ts",
            "https://example.test/x",
            "Application/XML; charset=utf-8",
            true,
        ),
        ("/play/a/foo.ts", "https://example.test/x", "text/xml", true),
        (
            "/play/a/foo.ts",
            "https://example.test/x",
            "application/xml+other",
            false,
        ),
    ] {
        assert_eq!(
            is_likely_manifest(local, target, Some(media)),
            expected,
            "{local} {media}"
        );
    }
    assert!(!is_likely_manifest("/play/a/seg.ts", "https://h/x.ts", None));
}

fn sample(method: &str, local: &str, target: &str, status: u16, header: i64, total: i64) -> TimingSample {
    TimingSample {
        method: method.to_owned(),
        local_path: local.to_owned(),
        target_url: target.to_owned(),
        status,
        header_ms: header,
        total_ms: total,
        bytes_out: 100,
        lazy_state: Some("HIT".to_owned()),
        lazy_wait_ms: -1,
        lazy_generator: None,
        failure: None,
    }
}

fn utc(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn hitch_golden_analyze() {
    let h = HitchDetector::new();
    let now = utc(1_790_769_600);
    let lazy = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000010.ts";
    assert!(
        h.analyze(&sample("POST", "/a/seg.ts", lazy, 500, 9000, 9000), now)
            .is_none()
    );
    assert!(
        h.analyze(
            &sample("GET", "/a/x.bin", "https://h/x.bin", 500, 9000, 9000),
            now
        )
        .is_none()
    );
    assert!(
        h.analyze(
            &sample("GET", "/a/seg.ts", "https://h/p/seg.ts", 200, 10, 20),
            now
        )
        .is_none()
    );
    assert!(
        h.analyze(
            &sample("get", "/a/m.m3u8", "https://h/p/m.m3u8", 200, 1999, 2499),
            now
        )
        .is_none()
    );

    let mut s = sample("GET", "/a/seg.ts", "https://h/p/seg.ts?x=1", 503, 1500, 2500);
    s.lazy_wait_ms = 1000;
    s.failure = Some("timeout".to_owned());
    let d = h.analyze(&s, now).unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(d.kind, "segment");
    assert_eq!(
        d.reasons,
        "segment-http-503,segment-timeout,slow-upstream-headers,slow-segment-total,server-generation-wait"
    );
    assert_eq!(
        (
            d.stream_id.as_str(),
            d.segment,
            d.previous_segment,
            d.gap_ms,
            d.header_ms,
            d.total_ms
        ),
        ("-", -1, -1, -1, 1500, 2500)
    );

    let d = h
        .analyze(&sample("GET", "/a/seg.m4s", "not a url", 200, 10, 2500), now)
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!((d.kind, d.reasons.as_str()), ("segment", "slow-segment-total"));

    let mut s = sample("GET", "/a/m.m3u8", "https://h/p/m.m3u8?q=1", 404, 2000, 2500);
    s.failure = Some("reset".to_owned());
    let d = h.analyze(&s, now).unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(d.kind, "manifest");
    assert_eq!(
        d.reasons,
        "manifest-http-404,manifest-reset,slow-manifest-headers,slow-manifest-total"
    );
    assert_eq!(d.stream_id, "-");

    let d = h
        .analyze(
            &sample("GET", "/a/index.mpd", "https://h/p/x", 200, 2000, 10),
            now,
        )
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (d.kind, d.reasons.as_str()),
        ("manifest", "slow-manifest-headers")
    );

    let d = h
        .analyze(&sample("GET", "/a/seg.ts", lazy, 200, 1600, 10), now)
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (
            d.kind,
            d.reasons.as_str(),
            d.stream_id.as_str(),
            d.segment,
            d.previous_segment,
            d.gap_ms
        ),
        ("segment", "slow-upstream-headers", "wk_s1", 10, -1, -1)
    );

    let skip = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000013.ts";
    let d = h
        .analyze(
            &sample("GET", "/a/seg.ts", skip, 200, 10, 10),
            now + Duration::from_secs(3),
        )
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (d.reasons.as_str(), d.previous_segment, d.gap_ms, d.segment),
        ("segment-skip", 10, 3000, 13)
    );

    let back = "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_s1/seg_000005.ts";
    let d = h
        .analyze(
            &sample("GET", "/a/seg.ts", back, 200, 10, 10),
            now + Duration::from_secs(4),
        )
        .unwrap_or_else(|| panic!("diagnostic"));
    assert_eq!(
        (d.reasons.as_str(), d.previous_segment),
        ("segment-backtrack", 13)
    );
}

#[test]
fn hitch_lazy_parse_golden() {
    for (url, expected) in [
        (
            "https://h/api/proxy/lazy-hls/wk_a/seg_000291.ts",
            Some(("wk_a", 291)),
        ),
        (
            "https://h/api/proxy/LAZY-HLS/wk_a/x/seg_7.ts?q=1",
            Some(("wk_a", 7)),
        ),
        ("https://h/api/proxy/lazy-hls/wk_a/index.m3u8", None),
        ("https://h/api/proxy/lazy-hls/seg_1.ts", None),
        ("relative/lazy-hls/wk_a/seg_1.ts", None),
        ("https://h/api/proxy/lazy-hls/wk_a/seg_.ts", None),
        ("https://h/api/proxy/lazy-hls/wk_a/seg_99999999999.ts", None),
    ] {
        let got = parse_lazy_segment(url);
        assert_eq!(got.as_ref().map(|(s, n)| (s.as_str(), *n)), expected, "{url}");
    }
}

#[test]
fn hitch_flags_segment_retries_but_not_steady_pacing() {
    let h = HitchDetector::new();
    let now = utc(1_778_454_000);
    let s = sample(
        "GET",
        "/play/a/seg_000042.ts",
        "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000042.ts",
        206,
        20,
        40,
    );
    assert!(h.analyze(&s, now).is_none());
    let retry = h
        .analyze(&s, now + Duration::from_secs(2))
        .unwrap_or_else(|| panic!("retry"));
    assert!(retry.reasons.contains("segment-retry"));
    assert_eq!(retry.previous_segment, 42);
    assert!(h.analyze(&s, now + Duration::from_secs(40)).is_none());

    let h = HitchDetector::new();
    let mut t = utc(1_790_706_412);
    for seg in 3..12 {
        let s = sample(
            "GET",
            &format!("/play/c629a1455fd0/seg_{seg:06}.ts"),
            &format!("https://proxy.whyknot.dev/api/proxy/lazy-hls/wk_fe89cc94bd139c30p/seg_{seg:06}.ts"),
            206,
            70,
            100,
        );
        assert!(h.analyze(&s, t).is_none(), "segment {seg}");
        t += Duration::from_millis(7200);
    }
}

#[test]
fn hitch_record_writes_the_file_line_or_the_console_line() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let log = Log::to_file(&dir.path().join("relay.log"));
    let (ux, out) = capture_ux();
    let h = HitchDetector::new();
    let mut s = sample(
        "GET",
        "/play/a/seg.ts",
        "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=secret",
        502,
        10,
        3000,
    );
    s.lazy_generator = Some("ffmpeg".to_owned());
    s.lazy_wait_ms = 5;
    s.failure = Some("body_idle_timeout".to_owned());
    h.record(&s, utc(1_790_769_600), &ux, &log);
    log.close();
    let text = fs::read_to_string(dir.path().join("relay.log")).unwrap_or_default();
    assert!(text.contains(
        "[relay][hitch] reasons=segment-http-502,segment-body_idle_timeout,slow-segment-total kind=segment stream=wk_abc segment=1 status=502 lazy=HIT lazy_wait_ms=5 lazy_generator=ffmpeg header_ms=10 total_ms=3000 bytes=100 failure=body_idle_timeout target=us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts"
    ), "{text}");
    assert!(!text.contains("secret"));
    assert!(out.text().is_empty());

    let log = Log::disabled();
    log.set_dev_console(true);
    let (ux, out) = capture_ux();
    HitchDetector::new().record(&s, utc(1_790_769_600), &ux, &log);
    assert!(out.text().contains(
        "hitch reasons=segment-http-502,segment-body_idle_timeout,slow-segment-total kind=segment segment=1 status=502 lazy_wait_ms=5 lazy_generator=ffmpeg total_ms=3000 failure=body_idle_timeout"
    ), "{}", out.text());
}
