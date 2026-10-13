#![allow(clippy::unwrap_used)]

use std::net::{IpAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Duration;

use vrcr_shared::{b64, fallback_binary, logutil, relay, url_guard};

#[test]
fn blocked_address_policy_blocks_loopback_private_and_link_local() {
    let cases = [
        ("127.0.0.1", true),
        ("127.0.0.2", true),
        ("0.0.0.0", true),
        ("10.0.0.5", true),
        ("169.254.169.254", true),
        ("172.16.0.1", true),
        ("172.31.255.255", true),
        ("192.168.1.1", true),
        ("::1", true),
        ("fe80::1", true),
        ("fc00::1", true),
        ("fd12:3456::1", true),
        ("::ffff:127.0.0.1", true),
        ("::ffff:192.168.1.1", true),
        ("8.8.8.8", false),
        ("172.32.0.1", false),
        ("2606:4700::1111", false),
    ];
    for (addr, expected) in cases {
        let ip: IpAddr = addr.parse().expect(addr);
        assert_eq!(url_guard::is_blocked(ip), expected, "{addr}");
    }
}

#[test]
fn is_safe_to_emit_allows_only_public_http_shapes() {
    let cases = [
        (
            "https://rr3---sn-4g5e6nsz.googlevideo.com/videoplayback?expire=1",
            true,
        ),
        ("https://proxy.whyknot.dev/api/proxy/manifest.m3u8?q=abc", true),
        ("https://vrcresolver.com/api/proxy/seg.ts?url=abc", true),
        (
            "http://localhost.youtube.com:41234/play/abc/manifest.m3u8?target=eA",
            true,
        ),
        ("http://93.184.216.34/video.mp4", true),
        ("file:///C:/Windows/System32/config/SAM", false),
        ("\\\\evil-host\\share\\video.mp4", false),
        ("rtmp://cdn.example.com/live", false),
        ("ftp://cdn.example.com/video.mp4", false),
        ("http://127.0.0.1/video.mp4", false),
        ("http://127.0.0.1:8080/video.mp4", false),
        ("http://[::1]/video.mp4", false),
        ("http://[::ffff:127.0.0.1]/video.mp4", false),
        ("http://10.0.0.5/video.mp4", false),
        ("http://169.254.169.254/latest/meta-data/", false),
        ("http://192.168.1.1/video.mp4", false),
        ("http://localhost/video.mp4", false),
        ("http://LOCALHOST:9000/video.mp4", false),
        ("http://foo.localhost/video.mp4", false),
        ("/relative/video.mp4", false),
        ("not a url", false),
        ("", false),
    ];
    for (url, expected) in cases {
        assert_eq!(url_guard::is_safe_to_emit(Some(url)), expected, "{url}");
    }
    assert!(!url_guard::is_safe_to_emit(None));
}

#[test]
fn avpro_compatibility_rejects_only_known_bad_shapes() {
    let cases = [
        ("rtmp://live.example.com/stream", false),
        ("rtmps://live.example.com/stream", false),
        ("https://cdn.example.com/video.flv", false),
        ("https://cdn.example.com/video.f4v", false),
        ("https://cdn.example.com/video.flv?token=abc", false),
        ("https://cdn.example.com/video.mp4?name=x.flv", true),
        ("https://cdn.example.com/master.m3u8", true),
        ("https://cdn.example.com/video.mp4", true),
        ("", false),
    ];
    for (url, expected) in cases {
        assert_eq!(url_guard::is_avpro_compatible_url(url), expected, "{url}");
    }
}

#[test]
fn relay_liveness_tracks_the_listener_lifecycle() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(relay::is_listening(port, Duration::from_millis(500)));
    drop(listener);
    assert!(!relay::is_listening(port, Duration::from_millis(500)));
    assert!(!relay::is_listening(0, Duration::from_millis(500)));
}

#[test]
fn terminal_failure_line_carries_both_reasons_and_timing() {
    let line = relay::terminal_failure_line(
        "abc12345",
        "youtube.com",
        "avpro",
        "validation_failed",
        "sign_in_required",
        12345,
    );
    assert_eq!(
        line,
        "terminal_failure request_id=abc12345 domain=youtube.com player=avpro \
         server_reason=validation_failed bundled_ytdlp_reason=sign_in_required elapsed_ms=12345"
    );
}

#[test]
fn bare_host_strips_www_and_falls_back_to_a_question_mark() {
    let cases = [
        ("https://www.youtube.com/watch?v=abc", "youtube.com"),
        ("https://youtube.com/watch?v=abc", "youtube.com"),
        ("https://WWW.Example.COM/x", "example.com"),
        ("https://wwwx.example.com/x", "wwwx.example.com"),
        ("not a url", "?"),
        ("", "?"),
    ];
    for (url, expected) in cases {
        assert_eq!(logutil::bare_host(url), expected, "{url}");
    }
}

#[test]
fn base64_round_trips_with_the_url_safe_alphabet_and_no_padding() {
    let value = "https://cdn.example.com/seg?sig=a+b/c=&t=1";
    let encoded = b64::encode(value);
    assert!(!encoded.contains(['+', '/', '=']));
    assert_eq!(b64::decode(&encoded).as_deref(), Some(value));
}

#[test]
fn base64_decode_accepts_padded_input_and_rejects_garbage() {
    assert_eq!(b64::decode("aGk=").as_deref(), Some("hi"));
    assert_eq!(b64::decode("!!!!"), None);
    assert_eq!(b64::decode(""), None);
    assert_eq!(b64::decode("   "), None);
}

const EXE: &str = r"C:\vrc\dist\tools";
const VRC: &str = r"C:\Users\u\AppData\LocalLow\VRChat\VRChat\Tools";

fn og(dir: &str) -> PathBuf {
    Path::new(dir).join("yt-dlp-og.exe")
}

fn ytdlp(dir: &str) -> PathBuf {
    Path::new(dir).join("yt-dlp.exe")
}

fn select(
    vrc: Option<&str>,
    exists: impl Fn(&Path) -> bool,
    ours: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    fallback_binary::select(Path::new(EXE), vrc.map(Path::new), exists, ours)
}

#[test]
fn fallback_prefers_og_next_to_the_wrapper() {
    let picked = select(Some(VRC), |p| p == og(EXE) || p == og(VRC), |_| false);
    assert_eq!(picked, Some(og(EXE)));
}

#[test]
fn fallback_uses_vrc_tools_og_when_the_local_og_is_missing() {
    assert_eq!(select(Some(VRC), |p| p == og(VRC), |_| false), Some(og(VRC)));
}

#[test]
fn fallback_uses_vrc_vanilla_ytdlp_when_no_og_exists() {
    assert_eq!(
        select(Some(VRC), |p| p == ytdlp(VRC), |_| false),
        Some(ytdlp(VRC))
    );
}

#[test]
fn fallback_skips_candidates_that_are_our_wrapper() {
    let picked = select(Some(VRC), |p| p == og(EXE) || p == ytdlp(VRC), |p| p == og(EXE));
    assert_eq!(picked, Some(ytdlp(VRC)));
}

#[test]
fn fallback_returns_none_when_nothing_usable_exists() {
    assert_eq!(select(Some(VRC), |_| false, |_| false), None);
    assert_eq!(select(Some(VRC), |p| p == ytdlp(VRC), |_| true), None);
}

#[test]
fn fallback_handles_a_missing_vrc_tools_dir() {
    assert_eq!(select(None, |p| p == og(EXE), |_| false), Some(og(EXE)));
    assert_eq!(select(None, |_| false, |_| false), None);
}
