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

#[test]
fn watchdog_matching_compares_full_paths_case_insensitively() {
    let dir = Path::new(r"C:\Users\me\AppData\Local\vrcresolver");
    let ours = Path::new(r"c:\users\ME\appdata\local\VRCRESOLVER\vrcresolver.exe");
    assert_eq!(match_watchdog(Some(ours), dir), WatchdogMatch::Ours);
    let legacy = Path::new(r"\\?\C:\Users\me\AppData\Local\vrcresolver\WKVRCProxy.exe");
    assert_eq!(match_watchdog(Some(legacy), dir), WatchdogMatch::Ours);
    let slashed = Path::new("C:/Users/me/AppData/Local/vrcresolver/vrcresolver.exe");
    assert_eq!(
        match_watchdog(
            Some(slashed),
            Path::new(r"C:\Users\me\AppData\Local\vrcresolver\")
        ),
        WatchdogMatch::Ours
    );
    let other = Path::new(r"D:\portable\vrcresolver.exe");
    assert_eq!(match_watchdog(Some(other), dir), WatchdogMatch::OtherInstall);
    let nested = Path::new(r"C:\Users\me\AppData\Local\vrcresolver\old\vrcresolver.exe");
    assert_eq!(match_watchdog(Some(nested), dir), WatchdogMatch::OtherInstall);
    assert_eq!(match_watchdog(None, dir), WatchdogMatch::Uninspectable);
    assert_eq!(
        match_watchdog(Some(Path::new("")), dir),
        WatchdogMatch::Uninspectable
    );
}

fn images(map: &[(u32, Option<&str>)]) -> HashMap<u32, Option<PathBuf>> {
    map.iter().map(|(pid, p)| (*pid, p.map(PathBuf::from))).collect()
}

#[test]
fn close_counts_every_kind_of_watchdog() {
    let dir = Path::new(r"C:\install");
    let found = images(&[
        (1, Some(r"C:\install\vrcresolver.exe")),
        (2, Some(r"D:\other\vrcresolver.exe")),
        (3, None),
        (4, Some(r"C:\install\WKVRCProxy.exe")),
    ]);
    let stopped = RefCell::new(Vec::new());
    let report = close_watchdogs(
        &[1, 2, 3, 4],
        dir,
        &|pid| found.get(&pid).cloned().flatten(),
        &|_| false,
        &|pid| {
            stopped.borrow_mut().push(pid);
            pid != 4
        },
    );
    assert_eq!(
        report,
        CloseReport {
            matched: 2,
            skipped: 1,
            exited: 1,
            uninspectable: 0,
            stuck: 1,
        }
    );
    assert_eq!(*stopped.borrow(), vec![1, 4]);
    assert_eq!(
        report.summary(),
        "[uninstall] close-watchdog matched=2 skipped_other_installs=1 already_exited=1 uninspectable=0"
    );
    assert_eq!(
        report.error().as_deref(),
        Some("1 VRCResolver watchdog process(es) did not exit -- close them and run the uninstaller again")
    );
}

#[test]
fn a_live_watchdog_we_cannot_inspect_blocks_the_uninstall() {
    let report = close_watchdogs(&[9], Path::new(r"C:\install"), &|_| None, &|_| true, &|_| true);
    assert_eq!(report.uninspectable, 1);
    assert!(report.error().unwrap_or_default().contains("can't inspect"));
    assert_eq!(CloseReport::default().error(), None);
}

#[test]
fn state_roots_cover_current_and_legacy_locations() {
    let paths = AppPaths::new(r"C:\Users\me\AppData\Local", r"C:\ProgramData");
    let roots: Vec<String> = state_roots(&paths)
        .iter()
        .map(|p| p.to_string_lossy().replace('/', "\\"))
        .collect();
    assert_eq!(
        roots,
        vec![
            r"C:\Users\me\AppData\LocalLow\vrcresolver",
            r"C:\Users\me\AppData\LocalLow\WKVRCProxy",
            r"C:\Users\me\AppData\Local\WKVRCProxy",
            r"C:\ProgramData\vrcresolver",
            r"C:\ProgramData\WKVRCProxy",
        ]
    );
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

#[test]
fn self_delete_only_removes_the_uninstaller_and_an_empty_folder() {
    assert_eq!(
        rmdir_log_name(&stamp()),
        "vrcresolver-uninstall-rmdir-20261004070509.log"
    );
    let dir = Path::new(r"C:\My Apps\vrcresolver");
    let exe = dir.join("vrcresolver.Uninstaller.exe");
    let args = self_delete_args(dir, &exe, Path::new(r"C:\Temp\x.log"));
    assert_eq!(
        args.as_deref(),
        Ok(
            r#"/c (ping 127.0.0.1 -n 4 > nul) & (del /f /q "C:\My Apps\vrcresolver\vrcresolver.Uninstaller.exe" & rmdir "C:\My Apps\vrcresolver") > "C:\Temp\x.log" 2>&1"#
        )
    );
    assert!(!args.unwrap_or_default().contains("/s"));
    let exe = Path::new(r"C:\a\u.exe");
    assert!(self_delete_args(Path::new("C:\\a\"b"), exe, Path::new("x")).is_err());
    assert!(self_delete_args(Path::new(r"C:\a"), exe, Path::new("x\"y")).is_err());
    assert!(self_delete_args(Path::new(r"C:\a"), Path::new("C:\\a\\u\".exe"), Path::new("x")).is_err());
    assert!(self_delete_args(Path::new(r"C:\%USERPROFILE%"), exe, Path::new("x")).is_err());
}

#[test]
fn install_files_merge_the_shipped_list_with_the_manifest() {
    let rel = |p: &str| p.split('/').collect::<PathBuf>();
    let bare = install_files(None);
    assert_eq!(bare.len(), SHIPPED_FILES.len());
    for name in SHIPPED_FILES {
        assert!(bare.contains(&rel(name)), "{name}");
    }
    let manifest = "aa\t1\tVRCResolver.exe\naa\t2\tdata/extra/new.bin\naa\t3\t../outside.txt\naa\t4\tC:\\evil.txt\nbroken\n";
    let files = install_files(Some(manifest));
    assert_eq!(files.len(), SHIPPED_FILES.len() + 1);
    assert!(files.contains(&rel("data/extra/new.bin")));
    assert!(!files.iter().any(|f| {
        let text = f.to_string_lossy();
        text.contains("outside") || text.contains("evil")
    }));
    assert_eq!(
        install_subdirs(&files),
        vec![rel("data/extra"), rel("data"), rel("tools")]
    );
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
            restore: (RestoreOutcome::Restored, "moved yt-dlp-og.exe back".to_owned()),
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
        fs::remove_file(path).map(|()| true).map_err(|e| e.to_string())
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
    Install { _dir: dir, path, exe }
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
    put(&inst.path.join("data/release-manifest.tsv"), &rows.join("\n"));
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

#[test]
fn clean_uninstall_runs_every_step_in_order() {
    let inst = install(true);
    let mut f = Fake::new();
    f.pids = vec![11];
    f.images = HashMap::from([(11, Some(inst.path.join("vrcresolver.exe")))]);
    f.hosts_entry = true;
    f.existing = RefCell::new(roots()[..2].iter().cloned().collect());

    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK, "{}", f.errs());
    let cmd = self_delete_cmd(&inst);
    assert_eq!(
        f.events(),
        vec![
            "stop 11".to_owned(),
            "settle".to_owned(),
            r"sweep C:\Tools".to_owned(),
            r"restore C:\Tools".to_owned(),
            r"sweep C:\Tools".to_owned(),
            "elevate vrcresolver.exe --remove-hosts-entry 10".to_owned(),
            "check --local-relay-tls-check".to_owned(),
            "elevate vrcresolver.exe --local-relay-tls-remove 30".to_owned(),
            "wininet".to_owned(),
            "close-log".to_owned(),
            cmd,
        ]
    );
    let out = f.out();
    for step in [
        "close-watchdog",
        "restore-yt-dlp",
        "remove-hosts",
        "remove-relay-tls",
        "restore-wininet-limit",
        "wipe-state",
        "remove-files",
        "schedule-self-delete",
    ] {
        assert!(out.contains(&format!("[uninstall] {step} start\n")), "{step}");
        assert!(out.contains(&format!("[uninstall] {step} ok")), "{step}");
    }
    assert!(out.contains("[uninstall] restore-yt-dlp Restored: moved yt-dlp-og.exe back"));
    assert!(out.contains("[uninstall] wipe-state directories_wiped=2"));
    assert!(out.contains("[uninstall] remove-files removed=1"));
    assert!(out.starts_with(&format!("[uninstall] start installDir={}", inst.path.display())));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("VRCResolver uninstalled. The install folder will disappear in a moment.")
    );
}

#[test]
fn unclosable_watchdog_stops_before_touching_anything() {
    let inst = install(true);
    let mut f = Fake::new();
    f.pids = vec![11];
    f.images = HashMap::from([(11, Some(inst.path.join("vrcresolver.exe")))]);
    f.stop_ok = false;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_ERRORS);
    assert_eq!(f.events(), vec!["stop 11".to_owned()]);
    assert!(
        f.errs()
            .starts_with("[uninstall] close-watchdog ERROR 1 VRCResolver watchdog"),
        "{}",
        f.errs()
    );
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("Uninstall stopped at close-watchdog -- see the message above.")
    );
}

#[test]
fn other_installs_are_left_running() {
    let inst = install(true);
    let mut f = Fake::new();
    f.pids = vec![5];
    f.images = HashMap::from([(5, Some(PathBuf::from(r"Z:\elsewhere\vrcresolver.exe")))]);
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(!f.events().iter().any(|e| e.starts_with("stop") || e == "settle"));
    assert!(f.out().contains("skipped_other_installs=1"));
}

#[test]
fn nothing_to_elevate_means_no_uac_prompt() {
    let inst = install(true);
    let mut f = Fake::new();
    f.check = Some(relay_tls::CHECK_CLEAN);
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(
        !f.events().iter().any(|e| e.starts_with("elevate")),
        "{:?}",
        f.events()
    );
    let out = f.out();
    assert!(out.contains("[uninstall] remove-hosts skipped: entry already absent"));
    assert!(
        out.contains("[uninstall] remove-relay-tls skipped: no relay certificate or legacy binding left")
    );
}

#[test]
fn a_failed_tls_check_still_elevates() {
    let inst = install(true);
    let mut f = Fake::new();
    f.check = None;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(
        f.events()
            .contains(&"elevate vrcresolver.exe --local-relay-tls-remove 30".to_owned())
    );
}

#[test]
fn missing_watchdog_exe_skips_both_elevated_steps() {
    let inst = install(false);
    let mut f = Fake::new();
    f.hosts_entry = true;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(
        !f.events()
            .iter()
            .any(|e| e.starts_with("elevate") || e.starts_with("check"))
    );
    let out = f.out();
    assert!(out.contains("[uninstall] remove-hosts skipped: watchdog exe missing (can't re-exec elevated)"));
    assert!(
        out.contains("[uninstall] remove-relay-tls skipped: watchdog exe missing (can't re-exec elevated)")
    );
}

#[test]
fn declined_uac_is_not_an_error_but_a_failed_child_is() {
    let inst = install(true);
    let mut f = Fake::new();
    f.hosts_entry = true;
    f.elevation = Elevated::Declined;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    let out = f.out();
    assert!(out.contains("UAC declined \u{2014} hosts entry left in place. Remove it manually if desired."));
    assert!(
        out.contains(
            "UAC declined -- localhost.youtube.com HTTPS certificate/bindings may be left in place."
        )
    );

    let inst = install(true);
    let mut f = Fake::new();
    f.elevation = Elevated::TimedOut;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(
        f.out()
            .contains("[uninstall] remove-relay-tls elevation child still running after 30s")
    );

    let inst = install(true);
    let mut f = Fake::new();
    f.hosts_entry = true;
    f.elevation = Elevated::Exited(1);
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_ERRORS);
    assert!(
        f.errs()
            .contains("[uninstall] remove-hosts ERROR elevated remove-hosts exited with code 1")
    );
    assert!(
        f.errs()
            .contains("[uninstall] remove-relay-tls ERROR elevated remove-relay-tls exited with code 1")
    );
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("Uninstall finished with 2 non-fatal error(s) \u{2014} see messages above.")
    );
}

#[test]
fn restore_outcomes_are_reported_like_the_csharp_uninstaller() {
    let inst = install(false);
    let mut f = Fake::new();
    f.restore = (RestoreOutcome::Failed, "access denied".to_owned());
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_ERRORS);
    assert!(f.errs().contains(
        "[uninstall] WARNING: yt-dlp.exe was not restored (access denied) -- check Tools/yt-dlp.exe by hand"
    ));
    assert!(f.errs().contains(
        "[uninstall] restore-yt-dlp ERROR Tools/yt-dlp.exe could not be restored -- access denied"
    ));
    assert_eq!(f.events().iter().filter(|e| e.starts_with("sweep")).count(), 2);

    let mut f = Fake::new();
    f.restore = (RestoreOutcome::RemovedOrphanWrapper, String::new());
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(
        f.out()
            .contains("removed our Tools/yt-dlp.exe so VRChat redownloads its yt-dlp on next session.")
    );

    let mut f = Fake::new();
    f.tools = None;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(
        f.out()
            .contains("[uninstall] restore-yt-dlp skipped: VRChat Tools dir not found")
    );
    assert!(!f.events().iter().any(|e| e.starts_with("restore")));
}

#[test]
fn wipe_and_wininet_failures_are_counted_but_do_not_stop_later_steps() {
    let inst = install(false);
    let mut f = Fake::new();
    f.wininet = Err("registry denied".to_owned());
    f.locked = HashSet::from([PathBuf::from(r"C:\pd")]);
    f.existing = RefCell::new(roots().into_iter().collect());
    f.spawn_ok = false;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_ERRORS);
    let errs = f.errs();
    assert!(errs.contains("[uninstall] restore-wininet-limit ERROR registry denied"));
    assert!(errs.contains(r"[uninstall] could not wipe C:\pd: access denied"));
    assert!(errs.contains("[uninstall] schedule-self-delete ERROR could not start cmd.exe: denied"));
    assert!(f.out().contains("[uninstall] wipe-state directories_wiped=2"));
    assert!(f.out().contains("[uninstall] wipe-state ok"));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("Uninstall finished with 2 non-fatal error(s) \u{2014} see messages above.")
    );
}

#[test]
fn extracted_into_a_shared_folder_only_our_files_go() {
    let inst = full_install(&["data/extra.bin"]);
    let user_files = [
        "holiday.jpg",
        "Projects/report.docx",
        "data/notes.txt",
        "tools/other.exe",
    ];
    for name in user_files {
        put(&inst.path.join(name), "mine");
    }
    let f = Fake::new();
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK, "{}", f.errs());

    for name in user_files {
        assert!(inst.path.join(name).is_file(), "{name} was deleted");
    }
    for name in SHIPPED_FILES.iter().chain(&["data/extra.bin"]) {
        if *name != "vrcresolver.Uninstaller.exe" {
            assert!(!inst.path.join(name).exists(), "{name} was left behind");
        }
    }
    assert_eq!(
        entries(&inst.path),
        vec![
            "Projects",
            "data",
            "holiday.jpg",
            "tools",
            "vrcresolver.Uninstaller.exe"
        ]
    );
    assert_eq!(f.events().last(), Some(&self_delete_cmd(&inst)));
    assert!(!f.events().iter().any(|e| e.contains("rmdir /s")));
    assert!(f.out().contains("[uninstall] remove-files removed=7"));
    assert_eq!(
        f.out.borrow().last().cloned(),
        Some(format!(
            "VRCResolver uninstalled. Files it didn't install were left in {}.",
            inst.path.display()
        ))
    );
}

#[test]
fn a_folder_with_only_our_files_is_emptied_for_the_self_delete() {
    let inst = full_install(&["data/extra/deep.bin"]);
    let f = Fake::new();
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK, "{}", f.errs());
    assert_eq!(entries(&inst.path), vec!["vrcresolver.Uninstaller.exe"]);
    assert_eq!(f.events().last(), Some(&self_delete_cmd(&inst)));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("VRCResolver uninstalled. The install folder will disappear in a moment.")
    );
}

#[test]
fn manifest_paths_outside_the_install_folder_are_never_touched() {
    let inst = full_install(&[]);
    let outside = inst
        .path
        .parent()
        .map(|p| p.join("outside.txt"))
        .unwrap_or_default();
    put(&outside, "mine");
    put(
        &inst.path.join("data/release-manifest.tsv"),
        "AAAA\t1\t../outside.txt\nAAAA\t1\tvrcresolver.exe\n",
    );
    let f = Fake::new();
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK, "{}", f.errs());
    assert!(outside.is_file());
    assert_eq!(entries(&inst.path), vec!["vrcresolver.Uninstaller.exe"]);
}

#[test]
fn a_file_we_cannot_remove_is_reported_and_the_folder_stays() {
    let inst = full_install(&[]);
    let mut f = Fake::new();
    f.locked = HashSet::from([inst.path.join("tools").join("yt-dlp.exe")]);
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_ERRORS);
    let errs = f.errs();
    assert!(errs.contains("[uninstall] could not remove"), "{errs}");
    assert!(
        errs.contains("[uninstall] remove-files ERROR 1 file(s) could not be removed"),
        "{errs}"
    );
    assert!(inst.path.join("tools").join("yt-dlp.exe").is_file());
    assert_eq!(entries(&inst.path), vec!["tools", "vrcresolver.Uninstaller.exe"]);
    assert_eq!(f.events().last(), Some(&self_delete_cmd(&inst)));
}
