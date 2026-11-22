use proptest::prelude::*;
use vrcr_shared::native_fallback::{MAX_ENTRIES, NativeFallbackHosts};

const KSYNC: &str = "ksync.arcanescripts.com";

fn hosts(entries: &[&str]) -> NativeFallbackHosts {
    NativeFallbackHosts::new(entries)
}

#[test]
fn exact_entry_matches_only_that_host() {
    let list = hosts(&[KSYNC]);
    assert!(list.matches_url("https://ksync.arcanescripts.com/video.mp4"));
    assert!(list.matches_url("http://ksync.arcanescripts.com"));
    assert!(!list.matches_url("https://evilksync.arcanescripts.com/video.mp4"));
    assert!(!list.matches_url("https://a.ksync.arcanescripts.com/video.mp4"));
    assert!(!list.matches_url("https://arcanescripts.com/video.mp4"));
    assert!(!list.matches_url("https://ksync.arcanescripts.com.evil.com/video.mp4"));
    assert!(!list.matches_url("https://ksync.arcanescripts.com.evil.test/video.mp4"));
    assert!(!list.matches_url("https://www.youtube.com/watch?v=abc"));
}

#[test]
fn request_urls_are_normalized_before_matching() {
    let list = hosts(&[KSYNC]);
    for url in [
        "HTTPS://KSync.ArcaneScripts.COM/Video",
        "https://ksync.arcanescripts.com:8443/video",
        "https://ksync.arcanescripts.com./video",
        "  https://ksync.arcanescripts.com/video  ",
        "https://user:secret@ksync.arcanescripts.com/video",
        "https://KSYNC.arcanescripts.com.:443/video",
    ] {
        assert!(list.matches_url(url), "{url}");
    }
}

#[test]
fn entries_are_normalized() {
    let list = hosts(&[
        "  KSync.ArcaneScripts.COM.  ",
        "ksync.arcanescripts.com:443",
        "https://ksync.arcanescripts.com/path",
    ]);
    assert_eq!(list.entries(), &[KSYNC.to_owned()]);
}

#[test]
fn wildcard_and_dot_entries_match_subdomains_only() {
    for entry in ["*.example.com", ".example.com", "*.EXAMPLE.com."] {
        let list = hosts(&[entry]);
        assert_eq!(list.entries(), &["*.example.com".to_owned()], "{entry}");
        assert!(list.matches_url("https://sub.example.com/"), "{entry}");
        assert!(list.matches_url("https://a.b.example.com/"), "{entry}");
        assert!(!list.matches_url("https://example.com/"), "{entry}");
        assert!(!list.matches_url("https://badexample.com/"), "{entry}");
        assert!(
            !list.matches_url("https://example.com.evil.test/"),
            "{entry}"
        );
    }
}

#[test]
fn apex_needs_its_own_exact_entry() {
    let list = hosts(&["*.example.com", "example.com"]);
    assert_eq!(
        list.entries(),
        &["*.example.com".to_owned(), "example.com".to_owned()]
    );
    assert!(list.matches_url("https://example.com/"));
    assert!(list.matches_url("https://sub.example.com/"));
}

#[test]
fn entries_split_on_commas_semicolons_and_whitespace() {
    let list = hosts(&["b.test, a.test;c.test\td.test\n e.test"]);
    assert_eq!(
        list.entries(),
        &["a.test", "b.test", "c.test", "d.test", "e.test"].map(str::to_owned)
    );
}

#[test]
fn entries_are_sorted_and_deduplicated() {
    let list = hosts(&[
        "z.test",
        "A.test",
        "a.test",
        "*.y.test",
        ".y.test",
        "a.test:80",
    ]);
    assert_eq!(
        list.entries(),
        &["*.y.test", "a.test", "z.test"].map(str::to_owned)
    );
}

#[test]
fn invalid_entries_are_ignored() {
    let list = hosts(&[
        "",
        "   ",
        "*",
        "*.",
        ".",
        "*.com",
        "*.10.0.0.1",
        "bad..host",
        "host/with/path",
        "user@host.test",
        "bad!host.test",
        "-leading.test",
        "-",
        ",;,",
    ]);
    assert_eq!(list.entries(), Vec::<String>::new());
    assert!(list.is_empty());
    assert!(!list.matches_url("https://com/"));
    assert!(!list.matches_url("https://anything.com/"));
}

#[test]
fn ip_entries_match_exactly() {
    let list = hosts(&["192.0.2.7", "[2001:db8::1]", "2001:db8::2"]);
    assert_eq!(
        list.entries(),
        &["192.0.2.7", "2001:db8::1", "2001:db8::2"].map(str::to_owned)
    );
    assert!(list.matches_url("http://192.0.2.7:8080/a"));
    assert!(list.matches_url("http://[2001:db8::1]/a"));
    assert!(list.matches_url("http://[2001:DB8:0::2]/a"));
    assert!(list.matches_url("http://[2001:db8::1]:8443/a"));
    assert!(!list.matches_url("http://192.0.2.70/a"));
    assert!(!list.matches_url("http://[2001:db8::3]/a"));
}

#[test]
fn invalid_request_urls_never_match() {
    let list = hosts(&[KSYNC]);
    for url in [
        "",
        "   ",
        "ksync.arcanescripts.com/video",
        "not a url",
        "file:///ksync.arcanescripts.com",
        "https://",
        "https://[ksync.arcanescripts.com]/",
        "https://ksync..arcanescripts.com/",
        "\u{0}\u{ffff}",
    ] {
        assert!(!list.matches_url(url), "{url:?}");
    }
}

#[test]
fn list_keeps_at_most_the_first_max_entries_hosts() {
    let wire: Vec<String> = (0..=MAX_ENTRIES)
        .map(|index| format!("h{index:04}.test"))
        .collect();
    let list = NativeFallbackHosts::new(&wire);
    assert_eq!(list.entries().len(), MAX_ENTRIES);
    assert!(list.matches_url(&format!("https://{}/", wire[MAX_ENTRIES - 1])));
    assert!(!list.matches_url(&format!("https://{}/", wire[MAX_ENTRIES])));
}

#[test]
fn packed_entries_count_against_the_cap() {
    let packed = (0..MAX_ENTRIES * 2)
        .map(|index| format!("p{index}.test"))
        .collect::<Vec<_>>()
        .join(",");
    let list = hosts(&[packed.as_str(), KSYNC]);
    assert_eq!(list.entries().len(), MAX_ENTRIES);
    assert!(!list.matches_url("https://ksync.arcanescripts.com/"));
}

#[test]
fn duplicates_and_invalid_entries_do_not_use_up_the_cap() {
    let mut wire = vec![KSYNC.to_owned(); MAX_ENTRIES * 2];
    wire.extend((0..MAX_ENTRIES).map(|_| "bad..host".to_owned()));
    wire.push("last.test".to_owned());
    let list = NativeFallbackHosts::new(&wire);
    assert_eq!(list.entries(), &[KSYNC.to_owned(), "last.test".to_owned()]);
}

#[test]
fn longest_valid_entry_is_kept_and_longer_ones_are_dropped() {
    let domain = format!("{0}.{0}.{0}.{1}", "a".repeat(63), "b".repeat(61));
    assert_eq!(domain.len(), 253);
    let longest = format!("*.{domain}.");
    let as_url = format!("https://{domain}/");
    let list = hosts(&[longest.as_str(), &as_url, &"c".repeat(100_000)]);
    assert_eq!(list.entries(), &[format!("*.{domain}")]);
}

#[test]
fn empty_list_matches_nothing() {
    let list = NativeFallbackHosts::default();
    assert!(list.is_empty());
    assert_eq!(list.entries(), Vec::<String>::new());
    assert!(!list.matches_url("https://ksync.arcanescripts.com/"));
}

proptest! {
    #[test]
    fn prefixed_hosts_never_match_an_exact_entry(prefix in "[a-z0-9-]{1,12}") {
        let list = hosts(&[KSYNC]);
        let url = format!("https://{prefix}{KSYNC}/");
        prop_assert!(!list.matches_url(&url));
    }

    #[test]
    fn subdomains_match_a_wildcard_entry(labels in proptest::collection::vec("[a-z0-9]{1,10}", 1..4)) {
        let list = hosts(&["*.example.com"]);
        let url = format!("https://{}.example.com/", labels.join("."));
        prop_assert!(list.matches_url(&url));
    }

    #[test]
    fn arbitrary_input_never_panics(entry in "\\PC{0,64}", url in "\\PC{0,64}") {
        let list = hosts(&[entry.as_str(), KSYNC]);
        let _ = list.matches_url(&url);
        let _ = list.matches_url(&format!("https://{url}"));
    }
}
