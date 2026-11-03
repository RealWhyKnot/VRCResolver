use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use vrcr_shared::logutil::UtcStamp;
use vrcr_shared::paths::AppPaths;
use vrcr_shared::relay_tls;
use vrcr_shared::tools::RestoreOutcome;
use vrcr_shared::win::Elevated;
use vrcr_uninstaller::flow::{self, EXIT_ERRORS, EXIT_OK, Host};
use vrcr_uninstaller::{
    CloseReport, SHIPPED_FILES, WatchdogMatch, close_watchdogs, install_files, install_subdirs,
    match_watchdog, rmdir_log_name, self_delete_args, state_roots,
};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"))
}

fn images(map: &[(u32, Option<&str>)]) -> HashMap<u32, Option<PathBuf>> {
    map.iter()
        .map(|(pid, p)| (*pid, p.map(PathBuf::from)))
        .collect()
}

fn stamp() -> UtcStamp {
    UtcStamp {
        year: 2026,
        month: 10,
        day: 4,
        hour: 7,
        minute: 5,
        second: 9,
        millis: 0,
    }
}

struct Fake {
    pids: Vec<u32>,
    images: HashMap<u32, Option<PathBuf>>,
    stop_ok: bool,
    tools: Option<PathBuf>,
    restore: (RestoreOutcome, String),
    hosts_entry: bool,
    elevation: Elevated,
    check: Option<i32>,
    wininet: Result<(), String>,
    existing: RefCell<HashSet<PathBuf>>,
    locked: HashSet<PathBuf>,
    spawn_ok: bool,
    out: RefCell<Vec<String>>,
    errs: RefCell<Vec<String>>,
    events: RefCell<Vec<String>>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            pids: Vec::new(),
            images: HashMap::new(),
            stop_ok: true,
            tools: Some(PathBuf::from(r"C:\Tools")),
            restore: (
                RestoreOutcome::Restored,
                "moved yt-dlp-og.exe back".to_owned(),
            ),
            hosts_entry: false,
            elevation: Elevated::Exited(0),
            check: Some(relay_tls::CHECK_NEEDS_REMOVAL),
            wininet: Ok(()),
            existing: RefCell::new(HashSet::new()),
            locked: HashSet::new(),
            spawn_ok: true,
            out: RefCell::new(Vec::new()),
            errs: RefCell::new(Vec::new()),
            events: RefCell::new(Vec::new()),
        }
    }

    fn event(&self, e: String) {
        self.events.borrow_mut().push(e);
    }

    fn out(&self) -> String {
        self.out.borrow().join("\n")
    }

    fn errs(&self) -> String {
        self.errs.borrow().join("\n")
    }

    fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }
}

impl Host for Fake {
    fn say(&self, line: &str) {
        self.out.borrow_mut().push(line.to_owned());
    }

    fn err(&self, line: &str) {
        self.errs.borrow_mut().push(line.to_owned());
    }

    fn watchdog_pids(&self) -> Vec<u32> {
        self.pids.clone()
    }

    fn image_path(&self, pid: u32) -> Option<PathBuf> {
        self.images.get(&pid).cloned().flatten()
    }

    fn alive(&self, _pid: u32) -> bool {
        true
    }

    fn stop(&self, pid: u32) -> bool {
        self.event(format!("stop {pid}"));
        self.stop_ok
    }

    fn settle(&self) {
        self.event("settle".to_owned());
    }

    fn tools_dir(&self) -> Option<PathBuf> {
        self.tools.clone()
    }

    fn sweep_tools(&self, dir: &Path) {
        self.event(format!("sweep {}", dir.display()));
    }

    fn restore_tools(&self, dir: &Path) -> (RestoreOutcome, String) {
        self.event(format!("restore {}", dir.display()));
        self.restore.clone()
    }

    fn hosts_has_entry(&self) -> bool {
        self.hosts_entry
    }

    fn elevate(&self, exe: &Path, arg: &str, timeout: Duration) -> Elevated {
        self.event(format!(
            "elevate {} {arg} {}",
            exe.file_name().unwrap_or_default().to_string_lossy(),
            timeout.as_secs()
        ));
        self.elevation.clone()
    }

    fn run_check(&self, _exe: &Path, arg: &str) -> Option<i32> {
        self.event(format!("check {arg}"));
        self.check
    }

    fn restore_wininet(&self) -> Result<(), String> {
        self.event("wininet".to_owned());
        self.wininet.clone()
    }

    fn close_log(&self) {
        self.event("close-log".to_owned());
    }

    fn remove_dir(&self, dir: &Path) -> Result<bool, String> {
        if self.locked.contains(dir) {
            return Err("access denied".to_owned());
        }
        Ok(self.existing.borrow_mut().remove(dir))
    }

    fn remove_file(&self, path: &Path) -> Result<bool, String> {
        if self.locked.contains(path) {
            return Err("access denied".to_owned());
        }
        if !path.is_file() {
            return Ok(false);
        }
        fs::remove_file(path)
            .map(|()| true)
            .map_err(|e| format!("remove fixture file: {e}"))
    }

    fn remove_empty_dir(&self, dir: &Path) -> bool {
        fs::remove_dir(dir).is_ok()
    }

    fn temp_dir(&self) -> PathBuf {
        PathBuf::from(r"C:\Temp")
    }

    fn utc_stamp(&self) -> UtcStamp {
        stamp()
    }

    fn spawn_hidden_cmd(&self, args: &str, cwd: &Path) -> Result<(), String> {
        self.event(format!("cmd {} in {}", args, cwd.display()));
        if self.spawn_ok {
            Ok(())
        } else {
            Err("could not start cmd.exe: denied".to_owned())
        }
    }
}

struct Install {
    _dir: tempfile::TempDir,
    path: PathBuf,
    exe: PathBuf,
}

fn put(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{e}"));
    }
    fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
}

fn install(with_watchdog: bool) -> Install {
    let dir = tmp();
    let path = dir.path().join("vrcresolver");
    let exe = path.join("vrcresolver.Uninstaller.exe");
    put(&exe, "uninstaller");
    if with_watchdog {
        put(&path.join("vrcresolver.exe"), "watchdog");
    }
    Install {
        _dir: dir,
        path,
        exe,
    }
}

fn full_install(extra_manifest_rows: &[&str]) -> Install {
    let inst = install(true);
    let mut rows = Vec::new();
    for name in SHIPPED_FILES.iter().chain(extra_manifest_rows) {
        if *name != "data/release-manifest.tsv" {
            put(&inst.path.join(name), name);
            rows.push(format!("AAAA\t1\t{name}"));
        }
    }
    put(
        &inst.path.join("data/release-manifest.tsv"),
        &rows.join("\n"),
    );
    inst
}

fn self_delete_cmd(inst: &Install) -> String {
    format!(
        r#"cmd /c (ping 127.0.0.1 -n 4 > nul) & (del /f /q "{}" & rmdir "{}") > "C:\Temp\vrcresolver-uninstall-rmdir-20261004070509.log" 2>&1 in C:\Temp"#,
        inst.exe.display(),
        inst.path.display()
    )
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{e}"))
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\state"),
        PathBuf::from(r"C:\legacy"),
        PathBuf::from(r"C:\pd"),
    ]
}

mod cleanup_cases;

mod flow_cases;

mod processes_cases;
