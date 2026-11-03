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
use vrcr_updater::flow::{
    self, EXIT_FAILED, EXIT_NO_SHA, EXIT_OK, EXIT_SHA_MISMATCH, Host, HttpText,
};
use vrcr_updater::install::{
    atomic_copy_over, ensure_staged_updater_copy, normalize_rel, read_manifest_paths,
    resolve_payload_root, safe_install_path,
};
use vrcr_updater::release::{
    asset_digest_sha, expected_sha, include_prereleases, integrity_sha, legacy_body_sha,
    parse_tag_version, safe_file_token, select_release,
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

fn release_doc(tag: &str, assets: &serde_json::Value) -> serde_json::Value {
    json!({"tag_name": tag, "assets": assets, "body": ""})
}

fn zip_asset(name: &str) -> serde_json::Value {
    json!({"name": name, "browser_download_url": format!("https://dl/{name}")})
}

fn manifest(files: &[&str]) -> String {
    files.iter().map(|f| format!("{SHA_A}\t1\t{f}\n")).collect()
}

fn build_zip(path: &Path, files: &[(&str, &str)]) {
    let file = fs::File::create(path).unwrap_or_else(|e| panic!("{e}"));
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in files {
        zip.start_file(*name, options)
            .unwrap_or_else(|e| panic!("{e}"));
        zip.write_all(text.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    zip.finish().unwrap_or_else(|e| panic!("{e}"));
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
        fs::write(dest, &self.zip_bytes).map_err(|e| format!("write fixture archive: {e}"))
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
                &manifest(&[
                    "vrcresolver.exe",
                    "vrcresolver.Updater.exe",
                    "tools/yt-dlp.exe",
                ]),
            ),
        ],
    );
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("{e}"));
    let sha = sha256_hex(&path).unwrap_or_default();
    fs::remove_file(&path).unwrap_or_else(|e| panic!("{e}"));
    (bytes, sha)
}

mod archives_cases;

mod flow_cases;

mod installation_cases;

mod releases_cases;
