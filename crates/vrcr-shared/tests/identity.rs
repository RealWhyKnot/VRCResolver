#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;
use vrcr_shared::wrapper_identity::{self, WrapperKind};
use vrcr_shared::{legacy, paths};

struct Scratch(TempDir);

impl Scratch {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let p = self.0.path().join(name);
        fs::write(&p, content).unwrap();
        p
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    fn dir(&self, name: &str) -> PathBuf {
        let p = self.path(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

fn pattern(len: usize, mul: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * mul) & 0xFF) as u8).collect()
}

fn with_marker_at(len: usize, at: usize, marker: &[u8]) -> Vec<u8> {
    let mut v = pattern(len, 31);
    v[at..at + marker.len()].copy_from_slice(marker);
    v
}

#[test]
fn contains_marker_finds_an_embedded_marker_only() {
    let s = Scratch::new();
    assert!(wrapper_identity::contains_marker(&s.file(
        "m.bin",
        &with_marker_at(4096, 1234, &wrapper_identity::marker())
    )));
    assert!(!wrapper_identity::contains_marker(
        &s.file("r.bin", &pattern(4096, 17))
    ));
    assert!(!wrapper_identity::contains_marker(&s.path("missing.bin")));
    assert!(!wrapper_identity::contains_marker(&s.file("e.bin", &[])));
}

#[test]
fn contains_marker_skips_files_over_the_scan_cap() {
    let s = Scratch::new();
    let len = usize::try_from(wrapper_identity::MAX_SCAN_BYTES).unwrap() + (1 << 20);
    let p = s.file("big.bin", &with_marker_at(len, 256, &wrapper_identity::marker()));
    assert!(!wrapper_identity::contains_marker(&p));
}

#[test]
fn classify_recognises_current_and_legacy_markers() {
    let s = Scratch::new();
    let ours = s.file(
        "ours.bin",
        &with_marker_at(8192, 256, &wrapper_identity::marker()),
    );
    assert_eq!(wrapper_identity::classify(&ours, None), WrapperKind::Ours);
    let old = s.file(
        "legacy.bin",
        &with_marker_at(8192, 512, &legacy::wrapper_marker()),
    );
    assert!(wrapper_identity::contains_marker(&old));
    assert_eq!(wrapper_identity::classify(&old, None), WrapperKind::Ours);
}

#[test]
fn marker_is_the_current_generation() {
    let m = wrapper_identity::marker();
    assert!(m.starts_with(b"VRCRESOLVER_WRAPPER_MARKER_v1:"));
    assert_ne!(m, legacy::wrapper_marker());
}

#[test]
fn classify_splits_unknown_binaries_by_size() {
    let s = Scratch::new();
    let big = usize::try_from(wrapper_identity::OURS_SIZE_CEILING).unwrap() + 4096;
    assert_eq!(
        wrapper_identity::classify(&s.file("large.bin", &pattern(big, 7)), None),
        WrapperKind::VrcBundledYtDlp
    );
    assert_eq!(
        wrapper_identity::classify(&s.file("small.bin", &pattern(3 << 20, 13)), None),
        WrapperKind::Unknown
    );
    assert_eq!(
        wrapper_identity::classify(&s.path("ghost.bin"), None),
        WrapperKind::Unknown
    );
}

#[test]
fn classify_trusts_the_known_hash_list() {
    let s = Scratch::new();
    let candidate = s.file("prior.bin", &pattern(2 << 20, 11));
    let sha = wrapper_identity::sha256_hex(&candidate).unwrap();
    let list = s.file(
        "wrapper_hashes.txt",
        format!("# old prior-release wrappers\n{sha}  2026.5.10.4  2026-05-10T00:00:00Z\n").as_bytes(),
    );
    assert_eq!(
        wrapper_identity::classify(&candidate, Some(&list)),
        WrapperKind::Ours
    );
    let other = s.file("nolist.bin", &pattern(3 << 20, 1));
    assert_eq!(
        wrapper_identity::classify(&other, Some(&s.path("no-such-list.txt"))),
        WrapperKind::Unknown
    );
}

#[test]
fn known_hash_list_ignores_comments_and_case() {
    let s = Scratch::new();
    let list = s.file(
        "list.txt",
        b"# header comment\n\n  \nAbC123  v1  2026-01-01\n# trailing\n",
    );
    assert!(wrapper_identity::known_hash_list_contains(&list, "abc123"));
    assert!(wrapper_identity::known_hash_list_contains(&list, "ABC123"));
    assert!(!wrapper_identity::known_hash_list_contains(&list, "deadbeef"));
}

#[test]
fn sha256_matches_the_known_empty_digest() {
    let s = Scratch::new();
    assert_eq!(
        wrapper_identity::sha256_hex(&s.file("empty.bin", &[])).as_deref(),
        Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
}

const PE_OFFSET: usize = 0x80;
const FIXTURE_LEN: usize = 64 * 1024;
const COOKIE_TRAILER: usize = 80;

fn directories(pe32_plus: bool) -> usize {
    PE_OFFSET + 24 + if pe32_plus { 112 } else { 96 }
}

fn skeleton(len: usize, pe32_plus: bool) -> Vec<u8> {
    let mut d: Vec<u8> = (0..len).map(|i| ((i * 7 + 1) & 0xFF) as u8).collect();
    let dirs = directories(pe32_plus);
    d[PE_OFFSET..dirs + 16 * 8].fill(0);
    d[0] = b'M';
    d[1] = b'Z';
    d[0x3C..0x40].copy_from_slice(&u32::try_from(PE_OFFSET).unwrap().to_le_bytes());
    d[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(b"PE\0\0");
    let magic: u16 = if pe32_plus { 0x20B } else { 0x10B };
    d[PE_OFFSET + 24..PE_OFFSET + 26].copy_from_slice(&magic.to_le_bytes());
    d
}

fn signed(len: usize, pe32_plus: bool) -> Vec<u8> {
    let mut d = skeleton(len, pe32_plus);
    let cert = 2048u32;
    let dirs = directories(pe32_plus);
    d[dirs + 32..dirs + 36].copy_from_slice(&(u32::try_from(len).unwrap() - cert).to_le_bytes());
    d[dirs + 36..dirs + 40].copy_from_slice(&cert.to_le_bytes());
    d
}

fn unsigned(len: usize, cookie: bool, pe32_plus: bool) -> Vec<u8> {
    let mut d = skeleton(len, pe32_plus);
    if cookie {
        let at = len - wrapper_identity::PYINSTALLER_COOKIE.len() - COOKIE_TRAILER;
        d[at..at + wrapper_identity::PYINSTALLER_COOKIE.len()]
            .copy_from_slice(wrapper_identity::PYINSTALLER_COOKIE);
        d[at + wrapper_identity::PYINSTALLER_COOKIE.len()..].fill(0);
    }
    d
}

fn cut(mut d: Vec<u8>, n: usize) -> Vec<u8> {
    d.truncate(d.len() - n);
    d
}

#[test]
fn looks_complete_follows_the_certificate_table_or_the_cookie() {
    let s = Scratch::new();
    let mut trailing = signed(FIXTURE_LEN, true);
    trailing.extend_from_slice(&[0u8; 16]);
    let cases: Vec<(&str, Vec<u8>, bool)> = vec![
        ("signed-complete", signed(FIXTURE_LEN, true), true),
        ("signed-cut-short", cut(signed(FIXTURE_LEN, true), 100), false),
        ("signed-pe32-complete", signed(FIXTURE_LEN, false), true),
        ("signed-with-trailing-bytes", trailing, false),
        ("unsigned-with-cookie", unsigned(FIXTURE_LEN, true, true), true),
        (
            "unsigned-pe32-with-cookie",
            unsigned(FIXTURE_LEN, true, false),
            true,
        ),
        (
            "unsigned-without-cookie",
            unsigned(FIXTURE_LEN, false, true),
            false,
        ),
        (
            "unsigned-cookie-cut-off",
            cut(unsigned(FIXTURE_LEN, true, true), COOKIE_TRAILER + 4),
            false,
        ),
        (
            "non-pe",
            (0..FIXTURE_LEN).map(|i| ((i * 7 + 1) & 0xFF) as u8).collect(),
            false,
        ),
        ("empty", Vec::new(), false),
        ("header-only", signed(FIXTURE_LEN, true)[..0x90].to_vec(), false),
    ];
    for (name, content, expected) in cases {
        let p = s.file(&format!("{name}.exe"), &content);
        assert_eq!(wrapper_identity::looks_complete(&p), expected, "{name}");
    }
    assert!(!wrapper_identity::looks_complete(&s.path("ghost.exe")));
}

#[test]
fn looks_complete_rejects_wild_offsets() {
    let s = Scratch::new();
    let mut wild = signed(FIXTURE_LEN, true);
    wild[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(!wrapper_identity::looks_complete(
        &s.file("wild-offset.exe", &wild)
    ));
    let mut cert = signed(FIXTURE_LEN, true);
    let dirs = directories(true);
    cert[dirs + 32..dirs + 36].copy_from_slice(&u32::MAX.to_le_bytes());
    cert[dirs + 36..dirs + 40].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(!wrapper_identity::looks_complete(&s.file("wild-cert.exe", &cert)));
}

#[test]
fn looks_complete_reads_a_file_still_open_for_writing() {
    use std::io::Write;
    let s = Scratch::new();
    let p = s.path("downloading.exe");
    let mut writer = fs::File::create(&p).unwrap();
    writer.write_all(&signed(FIXTURE_LEN, true)).unwrap();
    writer.flush().unwrap();
    assert!(wrapper_identity::looks_complete(&p));
}

fn quiet() -> impl FnMut(String) {
    |_| {}
}

#[test]
fn copy_legacy_root_copies_state_skips_logs_and_leaves_the_source() {
    let s = Scratch::new();
    let legacy = s.dir("legacy");
    fs::write(legacy.join("settings.json"), "{}").unwrap();
    fs::create_dir_all(legacy.join("crashes")).unwrap();
    fs::write(legacy.join("crashes").join("crash-1.log"), "boom").unwrap();
    fs::create_dir_all(legacy.join("logs")).unwrap();
    fs::write(legacy.join("logs").join("watchdog-1.log"), "noise").unwrap();
    let new_root = s.path("renamed");
    paths::copy_legacy_root(&legacy, &new_root, &mut quiet()).unwrap();
    assert_eq!(fs::read_to_string(new_root.join("settings.json")).unwrap(), "{}");
    assert_eq!(
        fs::read_to_string(new_root.join("crashes").join("crash-1.log")).unwrap(),
        "boom"
    );
    assert!(!new_root.join("logs").exists());
    assert!(new_root.join(paths::RENAME_MARKER).is_file());
    assert!(legacy.join("settings.json").is_file());
    assert!(legacy.join("logs").join("watchdog-1.log").is_file());
}

#[test]
fn copy_legacy_root_never_overwrites_and_is_idempotent() {
    let s = Scratch::new();
    let legacy = s.dir("legacy");
    fs::write(legacy.join("settings.json"), "OLD").unwrap();
    fs::write(legacy.join("client_id.txt"), "id-1").unwrap();
    let new_root = s.dir("renamed");
    fs::write(new_root.join("settings.json"), "NEW").unwrap();
    paths::copy_legacy_root(&legacy, &new_root, &mut quiet()).unwrap();
    assert_eq!(fs::read_to_string(new_root.join("settings.json")).unwrap(), "NEW");
    assert!(paths::renamed_root_already_populated(&new_root));
    fs::write(new_root.join("client_id.txt"), "id-2").unwrap();
    paths::copy_legacy_root(&legacy, &new_root, &mut quiet()).unwrap();
    assert_eq!(
        fs::read_to_string(new_root.join("client_id.txt")).unwrap(),
        "id-2"
    );
}

#[test]
fn renamed_root_gate_uses_the_marker_or_any_content() {
    let s = Scratch::new();
    assert!(!paths::renamed_root_already_populated(&s.path("missing")));
    let populated = s.dir("populated");
    assert!(!paths::renamed_root_already_populated(&populated));
    fs::write(populated.join("anything.txt"), "x").unwrap();
    assert!(paths::renamed_root_already_populated(&populated));
    let marked = s.dir("marked");
    fs::write(marked.join(paths::RENAME_MARKER), "2026-07-14").unwrap();
    assert!(paths::renamed_root_already_populated(&marked));
}

#[test]
fn migrate_program_data_copies_once_and_leaves_the_source() {
    let s = Scratch::new();
    let legacy = s.dir("pd-legacy");
    fs::write(legacy.join("localhost-youtube-relay-ports.txt"), "51234").unwrap();
    let new_root = s.path("pd-new");
    paths::migrate_program_data(&legacy, &new_root, &mut quiet()).unwrap();
    assert_eq!(
        fs::read_to_string(new_root.join("localhost-youtube-relay-ports.txt")).unwrap(),
        "51234"
    );
    assert!(legacy.join("localhost-youtube-relay-ports.txt").is_file());
    fs::write(legacy.join("extra.txt"), "late").unwrap();
    paths::migrate_program_data(&legacy, &new_root, &mut quiet()).unwrap();
    assert!(!new_root.join("extra.txt").exists());
}

#[test]
fn migrate_local_app_state_moves_state_and_plants_the_marker() {
    let s = Scratch::new();
    let source = s.dir("localapp");
    fs::write(source.join("settings.json"), "{}").unwrap();
    fs::create_dir_all(source.join("logs")).unwrap();
    fs::write(source.join("logs").join("old.log"), "noise").unwrap();
    let low = s.path("locallow-legacy");
    paths::migrate_legacy_local_app_state(&source, &low, &mut quiet());
    assert_eq!(fs::read_to_string(low.join("settings.json")).unwrap(), "{}");
    assert!(!low.join("logs").exists());
    assert!(low.join(paths::LOCALAPP_MARKER).is_file());
    assert!(!source.exists());
}

#[test]
fn local_low_sits_beside_local() {
    let p = paths::AppPaths::new(r"C:\Users\u\AppData\Local", r"C:\ProgramData");
    assert_eq!(p.local_low(), PathBuf::from(r"C:\Users\u\AppData\LocalLow"));
}

#[test]
fn env_fallback_prefers_the_new_prefix() {
    let both = |k: &str| match k {
        "VRCRESOLVER_X" => Some("new".to_owned()),
        "WKVRCPROXY_X" => Some("old".to_owned()),
        _ => None,
    };
    assert_eq!(legacy::env_with_fallback_in("X", both).as_deref(), Some("new"));
    let old_only = |k: &str| (k == "WKVRCPROXY_X").then(|| "old-opt-out".to_owned());
    assert_eq!(
        legacy::env_with_fallback_in("X", old_only).as_deref(),
        Some("old-opt-out")
    );
    assert_eq!(legacy::env_with_fallback_in("X", |_| None), None);
}
