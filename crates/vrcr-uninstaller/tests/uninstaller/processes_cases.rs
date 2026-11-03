use super::*;

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
    assert_eq!(
        match_watchdog(Some(other), dir),
        WatchdogMatch::OtherInstall
    );
    let nested = Path::new(r"C:\Users\me\AppData\Local\vrcresolver\old\vrcresolver.exe");
    assert_eq!(
        match_watchdog(Some(nested), dir),
        WatchdogMatch::OtherInstall
    );
    assert_eq!(match_watchdog(None, dir), WatchdogMatch::Uninspectable);
    assert_eq!(
        match_watchdog(Some(Path::new("")), dir),
        WatchdogMatch::Uninspectable
    );
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
        Some(
            "1 VRCResolver watchdog process(es) did not exit -- close them and run the uninstaller again"
        )
    );
}

#[test]
fn a_live_watchdog_we_cannot_inspect_blocks_the_uninstall() {
    let report = close_watchdogs(
        &[9],
        Path::new(r"C:\install"),
        &|_| None,
        &|_| true,
        &|_| true,
    );
    assert_eq!(report.uninspectable, 1);
    assert!(report.error().unwrap_or_default().contains("can't inspect"));
    assert_eq!(CloseReport::default().error(), None);
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
    assert!(
        !f.events()
            .iter()
            .any(|e| e.starts_with("stop") || e == "settle")
    );
    assert!(f.out().contains("skipped_other_installs=1"));
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
    assert!(out.contains(
        "[uninstall] remove-hosts skipped: watchdog exe missing (can't re-exec elevated)"
    ));
    assert!(out.contains(
        "[uninstall] remove-relay-tls skipped: watchdog exe missing (can't re-exec elevated)"
    ));
}
