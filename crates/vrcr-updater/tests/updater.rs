use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::json;
use vrcr_shared::release::{ANY_RELEASES_URL, STABLE_LATEST_URL, Version};
use vrcr_shared::wrapper_identity::sha256_hex;
use vrcr_updater::archive::{check_free_space, extract_zip};
use vrcr_updater::flow::{self, EXIT_FAILED, EXIT_NO_SHA, EXIT_OK, EXIT_SHA_MISMATCH, Host, HttpText};
use vrcr_updater::install::{
    atomic_copy_over, ensure_staged_updater_copy, normalize_rel, read_manifest_paths, resolve_payload_root,
    safe_install_path,
};
use vrcr_updater::release::{
    asset_digest_sha, expected_sha, include_prereleases, integrity_sha, legacy_body_sha, parse_tag_version,
    safe_file_token, select_release,
};
use vrcr_updater::{PromptPolicy, RELEASE_BUDGET, prompt_policy, temp, wait_for_release};

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"))
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{e}"));
    }
    fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{e}"))
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn no_log() -> impl FnMut(String) {
    |_| {}
}

fn release_doc(tag: &str, assets: serde_json::Value) -> serde_json::Value {
    json!({"tag_name": tag, "assets": assets, "body": ""})
}

fn zip_asset(name: &str) -> serde_json::Value {
    json!({"name": name, "browser_download_url": format!("https://dl/{name}")})
}

#[test]
fn tag_versions_follow_the_csharp_parser() {
    assert_eq!(
        parse_tag_version("v2026.10.14.0"),
        Ok(Version::parse("2026.10.14.0").unwrap_or(Version::ZERO))
    );
    assert_eq!(
        parse_tag_version("2026.10.14.3-C0A0"),
        Ok(Version::parse("2026.10.14.3").unwrap_or(Version::ZERO))
    );
    assert!(parse_tag_version("v1.2.3-beta").is_err());
    assert!(parse_tag_version("vv1.2").is_err());
    let err = parse_tag_version("garbage").err().unwrap_or_default();
    assert_eq!(err, "Could not parse latest tag: garbage");
}

#[test]
fn stable_release_takes_the_last_zip_and_the_integrity_asset() {
    let doc = release_doc(
        "v2026.10.1.0",
        json!([
            zip_asset("first.zip"),
            zip_asset("vrcresolver-2026.10.1.0.integrity.tsv"),
            json!({"name": "second.ZIP", "browser_download_url": "https://dl/second.ZIP", "digest": format!("sha256:{SHA_A}")}),
            zip_asset("notes.txt"),
        ]),
    );
    let mut lines = Vec::new();
    let r = select_release(&doc, false, &mut |l| lines.push(l)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.zip_name, "second.ZIP");
    assert_eq!(r.zip_url, "https://dl/second.ZIP");
    assert_eq!(r.zip_digest.as_deref(), Some(format!("sha256:{SHA_A}").as_str()));
    assert_eq!(
        r.integrity_url.as_deref(),
        Some("https://dl/vrcresolver-2026.10.1.0.integrity.tsv")
    );
    assert_eq!(lines.len(), 3);
}

#[test]
fn release_without_a_zip_or_assets_is_refused() {
    let none = release_doc("v1.0.0.0", json!([zip_asset("a.txt")]));
    assert_eq!(
        select_release(&none, false, &mut no_log()).err().as_deref(),
        Some("No .zip asset on latest release.")
    );
    let missing = json!({"tag_name": "v1.0.0.0"});
    assert!(select_release(&missing, false, &mut no_log()).is_err());
    let bad_tag = release_doc("nightly", json!([zip_asset("a.zip")]));
    assert!(select_release(&bad_tag, false, &mut no_log()).is_err());
}

#[test]
fn prerelease_mode_picks_the_highest_version() {
    let list = json!([
        {"tag_name": "v2026.9.1.0", "assets": [zip_asset("old.zip")]},
        {"tag_name": "v2026.10.2.0-ab12", "prerelease": true, "assets": [zip_asset("beta.zip")]},
        {"tag_name": "v2026.10.1.0", "assets": [zip_asset("mid.zip")]},
    ]);
    let r = select_release(&list, true, &mut no_log()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.zip_name, "beta.zip");
    assert_eq!(
        select_release(&json!([]), true, &mut no_log()).err().as_deref(),
        Some("No releases returned by GitHub.")
    );
}

#[test]
fn integrity_lines_are_matched_by_zip_name() {
    let text = format!("# sha\tsize\tname\r\n{SHA_B}\t12\tother.zip\r\n{SHA_A}\t34\tVRCResolver.zip\r\n");
    assert_eq!(
        integrity_sha(&text, "vrcresolver.zip"),
        Some(SHA_A.to_ascii_uppercase())
    );
    assert_eq!(integrity_sha(&text, "missing.zip"), None);
    assert_eq!(integrity_sha("nothex\t1\ta.zip", "a.zip"), None);
    assert_eq!(integrity_sha("", "a.zip"), None);
    assert_eq!(integrity_sha(&text, " "), None);
}

#[test]
fn asset_digest_and_body_fallbacks_parse_only_sha256() {
    assert_eq!(
        asset_digest_sha(Some(&format!(" SHA256:{SHA_A} "))),
        Some(SHA_A.to_ascii_uppercase())
    );
    assert_eq!(asset_digest_sha(Some(&format!("sha512:{SHA_A}"))), None);
    assert_eq!(asset_digest_sha(Some("sha256:abc")), None);
    assert_eq!(asset_digest_sha(None), None);
    let body = format!("Release notes\r\nsha256:  {SHA_B}  \r\nmore");
    assert_eq!(legacy_body_sha(&body), Some(SHA_B.to_ascii_uppercase()));
    assert_eq!(legacy_body_sha("SHA256: short"), None);
}

#[test]
fn expected_sha_prefers_integrity_then_digest_then_body() {
    let doc = release_doc(
        "v1.0.0.0",
        json!([
            json!({"name": "a.zip", "browser_download_url": "u", "digest": format!("sha256:{SHA_B}")}),
            zip_asset("a.integrity.tsv"),
        ]),
    );
    let mut r = select_release(&doc, false, &mut no_log()).unwrap_or_else(|e| panic!("{e}"));
    r.body = format!("SHA256: {SHA_B}");
    let from_integrity = expected_sha(&r, |_| Ok(format!("{SHA_A}\t1\ta.zip")), &mut no_log());
    assert_eq!(from_integrity, Some(SHA_A.to_ascii_uppercase()));

    let mut logged = Vec::new();
    let from_digest = expected_sha(&r, |_| Err("HTTP 404".to_owned()), &mut |l| logged.push(l));
    assert_eq!(from_digest, Some(SHA_B.to_ascii_uppercase()));
    assert_eq!(
        logged,
        vec!["[updater] integrity asset read failed: HTTP 404".to_owned()]
    );

    r.zip_digest = None;
    r.integrity_url = None;
    assert_eq!(
        expected_sha(&r, |_| panic!("no integrity url"), &mut no_log()),
        Some(SHA_B.to_ascii_uppercase())
    );
    r.body = String::new();
    assert_eq!(expected_sha(&r, |_| Ok(String::new()), &mut no_log()), None);
}

#[test]
fn prerelease_setting_is_read_from_settings_json() {
    assert!(include_prereleases(Some(
        r#"{"maintenance":{"include_prereleases":true}}"#
    )));
    assert!(!include_prereleases(Some(
        r#"{"maintenance":{"include_prereleases":"true"}}"#
    )));
    assert!(!include_prereleases(Some(r#"{"maintenance":{}}"#)));
    assert!(!include_prereleases(Some("not json")));
    assert!(!include_prereleases(None));
}

#[test]
fn tags_become_safe_file_names() {
    assert_eq!(safe_file_token("v2026.10.1.0-AB12"), "v2026.10.1.0-AB12");
    assert_eq!(safe_file_token("../x/y:z"), ".._x_y_z");
}

#[test]
fn relative_paths_are_normalized_and_escapes_refused() {
    assert_eq!(
        normalize_rel(r"tools\yt-dlp.exe"),
        Some(PathBuf::from("tools").join("yt-dlp.exe"))
    );
    assert_eq!(
        normalize_rel(" data//a.txt "),
        Some(PathBuf::from("data").join("a.txt"))
    );
    for bad in ["", "/abs", r"C:\x", "a/../b", "./a", "..", "   "] {
        assert_eq!(normalize_rel(bad), None, "{bad:?}");
    }
    let root = Path::new("install");
    assert_eq!(
        safe_install_path(root, "a/b.txt"),
        Ok(root.join("a").join("b.txt"))
    );
    assert_eq!(
        safe_install_path(root, "../x").err().as_deref(),
        Some("Unsafe release manifest path: ../x")
    );
}

#[test]
fn manifest_paths_skip_comments_and_bad_rows() {
    let dir = tmp();
    write(
        &dir.path().join("data/release-manifest.tsv"),
        &format!(
            "# header\n{SHA_A}\t1\tvrcresolver.exe\n{SHA_A}\t2\tTools/YT-DLP.exe\nbroken line\n{SHA_A}\t3\t../escape\n\n"
        ),
    );
    let paths = read_manifest_paths(dir.path());
    let keys: Vec<&String> = paths.keys().collect();
    assert_eq!(keys, vec!["tools/yt-dlp.exe", "vrcresolver.exe"]);
    assert_eq!(
        paths.get("tools/yt-dlp.exe").map(String::as_str),
        Some("Tools/YT-DLP.exe")
    );
    assert!(read_manifest_paths(&dir.path().join("missing")).is_empty());
}

#[test]
fn payload_root_is_the_zip_root_or_its_single_folder() {
    let dir = tmp();
    write(&dir.path().join("vrcresolver.exe"), "x");
    assert_eq!(resolve_payload_root(dir.path()), Ok(dir.path().to_path_buf()));

    let nested = tmp();
    write(&nested.path().join("pkg/WKVRCProxy.exe"), "x");
    write(&nested.path().join("other/readme.txt"), "x");
    assert_eq!(resolve_payload_root(nested.path()), Ok(nested.path().join("pkg")));

    let ambiguous = tmp();
    write(&ambiguous.path().join("a/vrcresolver.exe"), "x");
    write(&ambiguous.path().join("b/vrcresolver.exe"), "x");
    let err = resolve_payload_root(ambiguous.path()).err().unwrap_or_default();
    assert!(err.contains("recognizable payload root"), "{err}");
}

#[test]
fn staged_updater_copy_is_made_only_when_the_updater_ships() {
    let dir = tmp();
    assert_eq!(ensure_staged_updater_copy(dir.path()), Ok(()));
    assert!(names(dir.path()).is_empty());
    write(&dir.path().join("vrcresolver.Updater.exe"), "new updater");
    assert_eq!(ensure_staged_updater_copy(dir.path()), Ok(()));
    assert_eq!(
        read(&dir.path().join("vrcresolver.Updater.next.exe")),
        "new updater"
    );
}

fn manifest(files: &[&str]) -> String {
    files.iter().map(|f| format!("{SHA_A}\t1\t{f}\n")).collect()
}

#[test]
fn copy_over_replaces_files_skips_the_updater_and_drops_removed_ones() {
    let from = tmp();
    let to = tmp();
    write(&from.path().join("vrcresolver.exe"), "new watchdog");
    write(&from.path().join("vrcresolver.Updater.exe"), "new updater");
    write(&from.path().join("tools/yt-dlp.exe"), "new shim");
    write(
        &from.path().join("data/release-manifest.tsv"),
        &manifest(&["vrcresolver.exe", "vrcresolver.Updater.exe", "tools/yt-dlp.exe"]),
    );
    write(&to.path().join("vrcresolver.exe"), "old watchdog");
    write(&to.path().join("vrcresolver.Updater.exe"), "running updater");
    write(&to.path().join("tools/yt-dlp.exe"), "old shim");
    write(&to.path().join("tools/retired.dll"), "retired");
    write(&to.path().join("user-notes.txt"), "keep me");
    write(
        &to.path().join("data/release-manifest.tsv"),
        &manifest(&["vrcresolver.exe", "tools/yt-dlp.exe", "TOOLS/retired.dll"]),
    );

    atomic_copy_over(from.path(), to.path()).unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(read(&to.path().join("vrcresolver.exe")), "new watchdog");
    assert_eq!(
        read(&to.path().join("vrcresolver.Updater.exe")),
        "running updater"
    );
    assert_eq!(read(&to.path().join("tools/yt-dlp.exe")), "new shim");
    assert_eq!(read(&to.path().join("user-notes.txt")), "keep me");
    assert!(!to.path().join("tools/retired.dll").exists());
    assert_eq!(names(&to.path().join("tools")), vec!["yt-dlp.exe"]);
    assert_eq!(
        names(to.path()),
        vec![
            "data",
            "tools",
            "user-notes.txt",
            "vrcresolver.Updater.exe",
            "vrcresolver.exe"
        ]
    );
}

#[test]
fn copy_over_without_an_old_manifest_deletes_nothing() {
    let from = tmp();
    let to = tmp();
    write(&from.path().join("vrcresolver.exe"), "new");
    write(
        &from.path().join("data/release-manifest.tsv"),
        &manifest(&["vrcresolver.exe"]),
    );
    write(&to.path().join("extra.dll"), "kept");
    atomic_copy_over(from.path(), to.path()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(read(&to.path().join("extra.dll")), "kept");
}

#[test]
fn copy_over_rolls_back_every_swapped_file_when_one_cannot_be_replaced() {
    let from = tmp();
    let to = tmp();
    write(&from.path().join("a.txt"), "new a");
    write(&from.path().join("blocked"), "new file where a folder sits");
    write(&from.path().join("c.txt"), "new c");
    write(&to.path().join("a.txt"), "old a");
    write(&to.path().join("blocked/inner.txt"), "folder content");
    write(&to.path().join("c.txt"), "old c");

    let err = atomic_copy_over(from.path(), to.path()).err().unwrap_or_default();
    assert!(err.contains("Failed to replace"), "{err}");
    assert_eq!(read(&to.path().join("a.txt")), "old a");
    assert_eq!(read(&to.path().join("c.txt")), "old c");
    assert_eq!(read(&to.path().join("blocked/inner.txt")), "folder content");
    assert_eq!(names(to.path()), vec!["a.txt", "blocked", "c.txt"]);
}

#[cfg(windows)]
#[test]
fn copy_over_names_a_locked_yt_dlp_and_leaves_the_install_untouched() {
    use std::os::windows::fs::OpenOptionsExt;
    let from = tmp();
    let to = tmp();
    write(&from.path().join("tools/yt-dlp.exe"), "new shim");
    write(&from.path().join("vrcresolver.exe"), "new watchdog");
    write(&to.path().join("tools/yt-dlp.exe"), "old shim");
    write(&to.path().join("vrcresolver.exe"), "old watchdog");
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(to.path().join("tools/yt-dlp.exe"))
        .unwrap_or_else(|e| panic!("{e}"));

    let err = atomic_copy_over(from.path(), to.path()).err().unwrap_or_default();
    drop(lock);
    assert!(err.contains("file is locked. Close VRChat"), "{err}");
    assert_eq!(read(&to.path().join("tools/yt-dlp.exe")), "old shim");
    assert_eq!(read(&to.path().join("vrcresolver.exe")), "old watchdog");
    assert_eq!(names(&to.path().join("tools")), vec!["yt-dlp.exe"]);
    assert_eq!(names(to.path()), vec!["tools", "vrcresolver.exe"]);
}

fn build_zip(path: &Path, files: &[(&str, &str)]) {
    let file = fs::File::create(path).unwrap_or_else(|e| panic!("{e}"));
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in files {
        zip.start_file(*name, options).unwrap_or_else(|e| panic!("{e}"));
        zip.write_all(text.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
    }
    zip.finish().unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn zip_extraction_restores_the_tree() {
    let dir = tmp();
    let zip = dir.path().join("r.zip");
    build_zip(
        &zip,
        &[("pkg/vrcresolver.exe", "w"), ("pkg/tools/yt-dlp.exe", "y")],
    );
    let out = dir.path().join("out");
    extract_zip(&zip, &out).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(read(&out.join("pkg/tools/yt-dlp.exe")), "y");
    assert_eq!(resolve_payload_root(&out), Ok(out.join("pkg")));
}

#[test]
fn zip_extraction_never_writes_outside_the_target() {
    let dir = tmp();
    let zip = dir.path().join("evil.zip");
    build_zip(&zip, &[("../escaped.txt", "x"), ("ok.txt", "y")]);
    let out = dir.path().join("out");
    let _ = extract_zip(&zip, &out);
    assert!(!dir.path().join("escaped.txt").exists());
    assert!(
        !dir.path()
            .parent()
            .map(|p| p.join("escaped.txt").exists())
            .unwrap_or(false)
    );
}

#[test]
fn unreadable_zip_is_an_error() {
    let dir = tmp();
    let zip = dir.path().join("bad.zip");
    write(&zip, "not a zip");
    let err = extract_zip(&zip, &dir.path().join("out"))
        .err()
        .unwrap_or_default();
    assert!(err.starts_with("release zip is unreadable"), "{err}");
}

#[test]
fn free_space_needs_one_and_a_half_times_the_download() {
    let mib = 1024 * 1024;
    assert_eq!(check_free_space(100 * mib, Some(150 * mib), "C:\\"), Ok(()));
    assert_eq!(check_free_space(100 * mib, None, "C:\\"), Ok(()));
    assert_eq!(
        check_free_space(100 * mib, Some(149 * mib), "D:\\")
            .err()
            .as_deref(),
        Some("Insufficient free space on D:\\ for download (need ~150 MiB, have 149 MiB).")
    );
}

#[test]
fn temp_sweep_removes_only_stale_updater_artifacts() {
    let dir = tmp();
    for d in [
        "vrcresolver-extract-abc",
        "WKVRCProxy-extract-1",
        "vrcresolver-cache",
        "other-extract-1",
    ] {
        write(&dir.path().join(d).join("f.txt"), "x");
    }
    for f in [
        "vrcresolver-v1.zip",
        "WKVRCProxy-v1.zip",
        "vrcresolver-.zip",
        "notes.zip",
        "vrcresolver-v1.txt",
    ] {
        write(&dir.path().join(f), "x");
    }
    assert_eq!(temp::sweep(dir.path(), SystemTime::now()), 0);
    let later = SystemTime::now() + Duration::from_secs(2 * 24 * 3600);
    assert_eq!(temp::sweep(dir.path(), later), 5);
    assert_eq!(
        names(dir.path()),
        vec![
            "notes.zip",
            "other-extract-1",
            "vrcresolver-cache",
            "vrcresolver-v1.txt"
        ]
    );
}

#[test]
fn prompt_policy_matches_the_csharp_order() {
    assert_eq!(prompt_policy(Some("1"), true), PromptPolicy::Requested);
    assert_eq!(prompt_policy(Some("true"), true), PromptPolicy::Redirected);
    assert_eq!(prompt_policy(None, true), PromptPolicy::Redirected);
    assert_eq!(prompt_policy(None, false), PromptPolicy::Ask);
}

#[test]
fn release_wait_returns_once_processes_and_mutexes_are_gone() {
    let start = Instant::now();
    let clock = Cell::new(start);
    let polls = Cell::new(0u32);
    let left = || {
        polls.set(polls.get() + 1);
        polls.get() < 3
    };
    let sleep = |d: Duration| clock.set(clock.get() + d);
    let now = || clock.get();
    assert_eq!(wait_for_release(&left, &|| true, &sleep, &now), Ok(()));
    assert_eq!(clock.get() - start, Duration::from_millis(200));

    let clock = Cell::new(start);
    let sleep = |d: Duration| clock.set(clock.get() + d);
    let now = || clock.get();
    let waited = wait_for_release(&|| false, &|| false, &sleep, &now).err();
    assert_eq!(waited, Some(RELEASE_BUDGET));
}

struct Fake {
    install: PathBuf,
    temp: PathBuf,
    version: Option<String>,
    responses: HashMap<String, HttpText>,
    zip_bytes: Vec<u8>,
    answer: bool,
    prereleases: bool,
    out: RefCell<Vec<String>>,
    errs: RefCell<Vec<String>>,
    logs: RefCell<Vec<String>>,
    stopped: Cell<bool>,
    launched: RefCell<Option<(PathBuf, PathBuf)>>,
    prompted: Cell<bool>,
}

impl Fake {
    fn new(install: &Path, temp: &Path) -> Fake {
        Fake {
            install: install.to_path_buf(),
            temp: temp.to_path_buf(),
            version: Some("2026.10.1.0".to_owned()),
            responses: HashMap::new(),
            zip_bytes: Vec::new(),
            answer: true,
            prereleases: false,
            out: RefCell::new(Vec::new()),
            errs: RefCell::new(Vec::new()),
            logs: RefCell::new(Vec::new()),
            stopped: Cell::new(false),
            launched: RefCell::new(None),
            prompted: Cell::new(false),
        }
    }

    fn respond(&mut self, url: &str, status: u16, content_type: &str, body: &str) {
        self.responses.insert(
            url.to_owned(),
            HttpText {
                status,
                content_type: Some(content_type.to_owned()),
                body: body.to_owned(),
            },
        );
    }

    fn latest(&mut self, tag: &str, sha_line: &str) {
        let doc = json!({
            "tag_name": tag,
            "body": sha_line,
            "assets": [{"name": "vrcresolver.zip", "browser_download_url": "https://dl/vrcresolver.zip"}],
        });
        self.respond(
            STABLE_LATEST_URL,
            200,
            "application/json; charset=utf-8",
            &doc.to_string(),
        );
    }

    fn run(&self) -> i32 {
        flow::run(self, &self.install)
    }

    fn errs(&self) -> String {
        self.errs.borrow().join("\n")
    }
}

impl Host for Fake {
    fn say(&self, line: &str) {
        self.out.borrow_mut().push(line.to_owned());
    }

    fn err(&self, line: &str) {
        self.errs.borrow_mut().push(line.to_owned());
    }

    fn log(&self, line: &str) {
        self.logs.borrow_mut().push(line.to_owned());
    }

    fn file_version(&self, _path: &Path) -> Option<String> {
        self.version.clone()
    }

    fn include_prereleases(&self) -> bool {
        self.prereleases
    }

    fn get_text(&self, url: &str) -> Result<HttpText, String> {
        self.responses
            .get(url)
            .cloned()
            .ok_or_else(|| format!("no response for {url}"))
    }

    fn download(&self, _url: &str, dest: &Path) -> Result<(), String> {
        fs::write(dest, &self.zip_bytes).map_err(|e| e.to_string())
    }

    fn extract(&self, zip: &Path, dest: &Path) -> Result<(), String> {
        extract_zip(zip, dest)
    }

    fn prompt(&self) -> bool {
        self.prompted.set(true);
        self.answer
    }

    fn stop_watchdog(&self) {
        self.stopped.set(true);
    }

    fn launch_watchdog(&self, exe: &Path, dir: &Path) -> Result<(), String> {
        *self.launched.borrow_mut() = Some((exe.to_path_buf(), dir.to_path_buf()));
        Ok(())
    }

    fn temp_dir(&self) -> PathBuf {
        self.temp.clone()
    }
}

struct World {
    _root: tempfile::TempDir,
    install: PathBuf,
    temp: PathBuf,
}

fn world() -> World {
    let root = tmp();
    let install = root.path().join("install");
    let temp = root.path().join("temp");
    write(&install.join("vrcresolver.exe"), "old watchdog");
    write(&install.join("vrcresolver.Updater.exe"), "running updater");
    write(&install.join("tools/yt-dlp.exe"), "old shim");
    fs::create_dir_all(&temp).unwrap_or_else(|e| panic!("{e}"));
    World {
        _root: root,
        install,
        temp,
    }
}

fn release_zip(temp: &Path) -> (Vec<u8>, String) {
    let path = temp.join("built.zip");
    build_zip(
        &path,
        &[
            ("vrcresolver/vrcresolver.exe", "new watchdog"),
            ("vrcresolver/vrcresolver.Updater.exe", "new updater"),
            ("vrcresolver/tools/yt-dlp.exe", "new shim"),
            (
                "vrcresolver/data/release-manifest.tsv",
                &manifest(&["vrcresolver.exe", "vrcresolver.Updater.exe", "tools/yt-dlp.exe"]),
            ),
        ],
    );
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("{e}"));
    let sha = sha256_hex(&path).unwrap_or_default();
    fs::remove_file(&path).unwrap_or_else(|e| panic!("{e}"));
    (bytes, sha)
}

#[test]
fn up_to_date_install_exits_cleanly_without_prompting() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.1.0", "");
    assert_eq!(f.run(), EXIT_OK);
    assert_eq!(
        *f.out.borrow(),
        vec![
            "Current version: 2026.10.1.0".to_owned(),
            "You're on the latest version.".to_owned()
        ]
    );
    assert!(!f.prompted.get());
}

#[test]
fn missing_watchdog_fails_with_the_install_dir_hint() {
    let w = world();
    fs::remove_file(w.install.join("vrcresolver.exe")).unwrap_or_else(|e| panic!("{e}"));
    let f = Fake::new(&w.install, &w.temp);
    assert_eq!(f.run(), EXIT_FAILED);
    assert!(
        f.errs()
            .starts_with("Updater failed: vrcresolver.exe was not found next to the updater"),
        "{}",
        f.errs()
    );
}

#[test]
fn unreadable_file_version_counts_as_zero() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.version = Some("2026.10.1.0-ABCD".to_owned());
    f.latest("v0.0.0.1", "");
    f.answer = false;
    assert_eq!(f.run(), EXIT_NO_SHA);
    assert_eq!(f.out.borrow()[0], "Current version: 0.0.0.0");
}

#[test]
fn release_without_a_checksum_is_refused_before_prompting() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.2.0", "no checksum here");
    assert_eq!(f.run(), EXIT_NO_SHA);
    assert_eq!(
        f.errs(),
        "Refusing to update: release did not provide a zip SHA256."
    );
    assert!(!f.prompted.get());
}

#[test]
fn declined_prompt_skips_without_touching_the_install() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.2.0", &format!("SHA256: {SHA_A}"));
    f.answer = false;
    assert_eq!(f.run(), EXIT_OK);
    assert_eq!(f.out.borrow().last().map(String::as_str), Some("Skipped."));
    assert!(names(&w.temp).is_empty());
    assert!(!f.stopped.get());
}

#[test]
fn checksum_mismatch_refuses_and_deletes_the_download() {
    let w = world();
    let (bytes, _) = release_zip(&w.temp);
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.2.0", &format!("SHA256: {SHA_A}"));
    f.zip_bytes = bytes;
    assert_eq!(f.run(), EXIT_SHA_MISMATCH);
    assert!(
        f.errs().starts_with(
            "Refusing to install: SHA256 mismatch.\n  url=https://dl/vrcresolver.zip\n  expected="
        ),
        "{}",
        f.errs()
    );
    assert!(names(&w.temp).is_empty());
    assert!(!f.stopped.get());
    assert_eq!(read(&w.install.join("vrcresolver.exe")), "old watchdog");
}

#[test]
fn full_update_installs_stages_the_updater_and_relaunches() {
    let w = world();
    let (bytes, sha) = release_zip(&w.temp);
    let mut f = Fake::new(&w.install, &w.temp);
    f.latest("v2026.10.2.0", &format!("SHA256: {}", sha.to_ascii_lowercase()));
    f.zip_bytes = bytes;
    assert_eq!(f.run(), EXIT_OK, "{}", f.errs());
    assert!(f.stopped.get());
    assert_eq!(
        *f.launched.borrow(),
        Some((w.install.join("vrcresolver.exe"), w.install.clone()))
    );
    assert_eq!(read(&w.install.join("vrcresolver.exe")), "new watchdog");
    assert_eq!(read(&w.install.join("tools/yt-dlp.exe")), "new shim");
    assert_eq!(
        read(&w.install.join("vrcresolver.Updater.exe")),
        "running updater"
    );
    assert_eq!(
        read(&w.install.join("vrcresolver.Updater.next.exe")),
        "new updater"
    );
    assert!(names(&w.temp).is_empty(), "{:?}", names(&w.temp));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("Update installed. Relaunching watchdog...")
    );
    assert!(
        f.logs
            .borrow()
            .iter()
            .any(|l| l == "[updater] expected-sha256 matched (64 chars)")
    );
}

#[test]
fn prerelease_setting_switches_to_the_release_list() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.prereleases = true;
    let list = json!([{"tag_name": "v2026.10.1.0", "assets": [zip_asset("a.zip")]}]);
    f.respond(ANY_RELEASES_URL, 200, "application/json", &list.to_string());
    assert_eq!(f.run(), EXIT_OK);
    assert!(
        f.logs
            .borrow()
            .iter()
            .any(|l| l.ends_with("(include-prereleases=on)"))
    );
}

#[test]
fn non_json_and_error_responses_fail_with_a_clear_message() {
    let w = world();
    let mut f = Fake::new(&w.install, &w.temp);
    f.respond(STABLE_LATEST_URL, 200, "text/html", "<html>maintenance</html>");
    assert_eq!(f.run(), EXIT_FAILED);
    assert!(
        f.errs()
            .contains("returned non-JSON content (Content-Type: text/html)"),
        "{}",
        f.errs()
    );

    let mut f = Fake::new(&w.install, &w.temp);
    f.respond(STABLE_LATEST_URL, 503, "application/json", "{}");
    assert_eq!(f.run(), EXIT_FAILED);
    assert_eq!(f.errs(), "Updater failed: GitHub releases API returned HTTP 503.");
}
