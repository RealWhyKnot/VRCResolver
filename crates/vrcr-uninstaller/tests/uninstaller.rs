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
    CloseReport, WatchdogMatch, close_watchdogs, match_watchdog, rmdir_log_name, self_delete_args,
    state_roots,
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
fn self_delete_command_matches_the_csharp_shape_and_refuses_quotes() {
    assert_eq!(
        rmdir_log_name(&stamp()),
        "vrcresolver-uninstall-rmdir-20261004070509.log"
    );
    let args = self_delete_args(Path::new(r"C:\My Apps\vrcresolver"), Path::new(r"C:\Temp\x.log"));
    assert_eq!(
        args.as_deref(),
        Ok(
            r#"/c (ping 127.0.0.1 -n 4 > nul) & (rmdir /s /q "C:\My Apps\vrcresolver") > "C:\Temp\x.log" 2>&1"#
        )
    );
    assert!(self_delete_args(Path::new("C:\\a\"b"), Path::new("x")).is_err());
    assert!(self_delete_args(Path::new("C:\\a"), Path::new("x\"y")).is_err());
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
}

fn install(with_watchdog: bool) -> Install {
    let dir = tmp();
    let path = dir.path().join("vrcresolver");
    fs::create_dir_all(&path).unwrap_or_else(|e| panic!("{e}"));
    if with_watchdog {
        fs::write(path.join("vrcresolver.exe"), "watchdog").unwrap_or_else(|e| panic!("{e}"));
    }
    Install { _dir: dir, path }
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

    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK, "{}", f.errs());
    let cmd = format!(
        r#"cmd /c (ping 127.0.0.1 -n 4 > nul) & (rmdir /s /q "{}") > "C:\Temp\vrcresolver-uninstall-rmdir-20261004070509.log" 2>&1 in C:\Temp"#,
        inst.path.display()
    );
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
        "schedule-self-delete",
    ] {
        assert!(out.contains(&format!("[uninstall] {step} start\n")), "{step}");
        assert!(out.contains(&format!("[uninstall] {step} ok")), "{step}");
    }
    assert!(out.contains("[uninstall] restore-yt-dlp Restored: moved yt-dlp-og.exe back"));
    assert!(out.contains("[uninstall] wipe-state directories_wiped=2"));
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
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_ERRORS);
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
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
    assert!(!f.events().iter().any(|e| e.starts_with("stop") || e == "settle"));
    assert!(f.out().contains("skipped_other_installs=1"));
}

#[test]
fn nothing_to_elevate_means_no_uac_prompt() {
    let inst = install(true);
    let mut f = Fake::new();
    f.check = Some(relay_tls::CHECK_CLEAN);
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
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
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
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
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
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
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
    let out = f.out();
    assert!(out.contains("UAC declined \u{2014} hosts entry left in place. Remove it manually if desired."));
    assert!(
        out.contains(
            "UAC declined -- localhost.youtube.com HTTPS certificate/bindings may be left in place."
        )
    );

    let mut f = Fake::new();
    f.elevation = Elevated::TimedOut;
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
    assert!(
        f.out()
            .contains("[uninstall] remove-relay-tls elevation child still running after 30s")
    );

    let mut f = Fake::new();
    f.hosts_entry = true;
    f.elevation = Elevated::Exited(1);
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_ERRORS);
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
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_ERRORS);
    assert!(f.errs().contains(
        "[uninstall] WARNING: yt-dlp.exe was not restored (access denied) -- check Tools/yt-dlp.exe by hand"
    ));
    assert!(f.errs().contains(
        "[uninstall] restore-yt-dlp ERROR Tools/yt-dlp.exe could not be restored -- access denied"
    ));
    assert_eq!(f.events().iter().filter(|e| e.starts_with("sweep")).count(), 2);

    let mut f = Fake::new();
    f.restore = (RestoreOutcome::RemovedOrphanWrapper, String::new());
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
    assert!(
        f.out()
            .contains("removed our Tools/yt-dlp.exe so VRChat redownloads its yt-dlp on next session.")
    );

    let mut f = Fake::new();
    f.tools = None;
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_OK);
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
    assert_eq!(flow::run(&f, &inst.path, &roots()), EXIT_ERRORS);
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
