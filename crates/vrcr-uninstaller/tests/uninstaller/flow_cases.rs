use super::*;

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
        assert!(
            out.contains(&format!("[uninstall] {step} start\n")),
            "{step}"
        );
        assert!(out.contains(&format!("[uninstall] {step} ok")), "{step}");
    }
    assert!(out.contains("[uninstall] restore-yt-dlp Restored: moved yt-dlp-og.exe back"));
    assert!(out.contains("[uninstall] wipe-state directories_wiped=2"));
    assert!(out.contains("[uninstall] remove-files removed=1"));
    assert!(out.starts_with(&format!(
        "[uninstall] start installDir={}",
        inst.path.display()
    )));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("VRCResolver uninstalled. The install folder will disappear in a moment.")
    );
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
    assert!(out.contains(
        "[uninstall] remove-relay-tls skipped: no relay certificate or legacy binding left"
    ));
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
fn declined_uac_is_not_an_error_but_a_failed_child_is() {
    let inst = install(true);
    let mut f = Fake::new();
    f.hosts_entry = true;
    f.elevation = Elevated::Declined;
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    let out = f.out();
    assert!(out.contains(
        "UAC declined \u{2014} hosts entry left in place. Remove it manually if desired."
    ));
    assert!(out.contains(
        "UAC declined -- localhost.youtube.com HTTPS certificate/bindings may be left in place."
    ));

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
    assert!(f.errs().contains(
        "[uninstall] remove-relay-tls ERROR elevated remove-relay-tls exited with code 1"
    ));
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
    assert_eq!(
        f.events().iter().filter(|e| e.starts_with("sweep")).count(),
        2
    );

    let mut f = Fake::new();
    f.restore = (RestoreOutcome::RemovedOrphanWrapper, String::new());
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK);
    assert!(f.out().contains(
        "removed our Tools/yt-dlp.exe so VRChat redownloads its yt-dlp on next session."
    ));

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
    assert!(
        errs.contains("[uninstall] schedule-self-delete ERROR could not start cmd.exe: denied")
    );
    assert!(
        f.out()
            .contains("[uninstall] wipe-state directories_wiped=2")
    );
    assert!(f.out().contains("[uninstall] wipe-state ok"));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("Uninstall finished with 2 non-fatal error(s) \u{2014} see messages above.")
    );
}
