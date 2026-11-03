use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::{install_manifest, logutil, trust_gateway};

#[test]
fn url_logs_keep_only_the_host() {
    for source in [
        "https://user:password@example.com/private-key/video.mp4?token=secret#credential",
        "https://example.com/another-private-path",
    ] {
        assert_eq!(logutil::redact_url(source), "example.com");
    }
    for source in [
        "bad-url?token=secret",
        "file:///private/key",
        "https://[secret",
    ] {
        assert_eq!(logutil::redact_url(source), "?");
    }
    assert_eq!(logutil::redact_url(""), "");
}

#[test]
fn console_messages_redact_embedded_urls_before_truncation() {
    assert_eq!(
        logutil::sanitize_for_console(
            "failed https://user:password@example.com/private?key=secret then HTTP://other.test/private",
            120,
        ),
        "failed example.com then other.test"
    );
    assert_eq!(
        logutil::sanitize_for_console("\"https://example.com/private?key=secret\"", 120),
        "\"example.com\""
    );
}

#[test]
fn file_logs_redact_urls_and_keep_each_event_on_one_line() {
    let directory = tempfile::tempdir().expect("temporary directory is available");
    let path = directory.path().join("test.log");
    let log = vrcr_shared::log::Log::to_file(&path);
    log.line("request https://user:password@example.com/private?key=secret\ninjected");
    log.close();
    let text = std::fs::read_to_string(path).expect("log was written");
    assert!(text.contains("request example.com?injected"), "{text}");
    assert_eq!(text.lines().count(), 1);
}

#[test]
fn unspecified_and_multicast_addresses_are_not_public_media_targets() {
    for address in [
        "::",
        "ff02::1",
        "224.0.0.1",
        "255.255.255.255",
        "100.64.0.1",
    ] {
        assert!(
            vrcr_shared::url_guard::is_blocked(address.parse().expect("fixed IP address")),
            "{address}"
        );
    }
}

#[test]
fn manifest_paths_reject_windows_aliases_and_control_characters() {
    for path in [
        ".. /outside.exe",
        "data./outside.exe",
        "data /outside.exe",
        "NUL.exe",
        "data/COM1.txt",
        "data/LPT9",
        "data/aux",
        "data/CONIN$",
        "data/com\u{b9}.txt",
        "data/bad\0name",
        "data/bad\nname",
        "data/name?.txt",
        "data/name*.txt",
    ] {
        assert_eq!(install_manifest::normalize_rel(path), None, "{path:?}");
    }
    assert_eq!(
        install_manifest::normalize_rel("data/complete.txt"),
        Some(std::path::PathBuf::from("data/complete.txt"))
    );
}

#[test]
fn gateway_paths_end_in_the_handler_extension_and_keep_the_original_target() {
    let policy = FirstPartyPolicy::default();
    for (target, extension) in [
        (
            "https://vrcresolver.com/api/proxy/video.mp4?token=secret",
            "mp4",
        ),
        (
            "https://vrcresolver.com/api/proxy/video.m3u8?token=secret",
            "m3u8",
        ),
        (
            "https://vrcresolver.com/api/proxy/video.unknown?token=secret",
            "bin",
        ),
    ] {
        let local = trust_gateway::build(&policy, 51234, target, Some("session"), "http")
            .expect("first-party target and relay port are valid");
        let parsed = url::Url::parse(&local).expect("gateway URL is valid");
        let segment = parsed
            .path()
            .rsplit('/')
            .next()
            .expect("gateway path exists");
        assert_eq!(
            std::path::Path::new(segment)
                .extension()
                .and_then(|ext| ext.to_str()),
            Some(extension)
        );
        assert_eq!(parsed.fragment(), None);
        assert_eq!(
            trust_gateway::extract_target(&local).as_deref(),
            Some(target)
        );
    }
}
