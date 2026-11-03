use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_watchdog::relay::{manifest, short_url};

#[test]
fn relay_diagnostics_do_not_expose_url_credentials_paths_or_queries() {
    assert_eq!(
        short_url("https://user:password@example.com/stream-key?token=secret#private"),
        "example.com"
    );
    assert_eq!(short_url("invalid?token=secret"), "?");
    assert_eq!(short_url(""), "?");
}

#[test]
fn rewritten_manifest_urls_end_in_known_extensions() {
    let policy = FirstPartyPolicy::default();
    for (source, extension) in [
        (
            "https://vrcresolver.com/api/proxy/segment.ts?key=secret",
            "ts",
        ),
        (
            "https://vrcresolver.com/api/proxy/segment.unknown?key=secret",
            "bin",
        ),
    ] {
        let local = manifest::build_local_target(&policy, source)
            .expect("first-party playback targets can be localized");
        let base = url::Url::parse("http://localhost.youtube.com:51234/play/testsession/")
            .expect("base URL is valid");
        let parsed = base.join(&local).expect("localized URL is valid");
        let segment = parsed
            .path()
            .rsplit('/')
            .next()
            .expect("localized path exists");
        assert_eq!(
            std::path::Path::new(segment)
                .extension()
                .and_then(|ext| ext.to_str()),
            Some(extension)
        );
        assert_eq!(parsed.fragment(), None);
        assert_eq!(
            vrcr_shared::trust_gateway::extract_target(parsed.as_str()).as_deref(),
            Some(source)
        );
    }
}
