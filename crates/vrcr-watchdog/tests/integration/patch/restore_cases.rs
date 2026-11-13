use super::*;

#[test]
fn missing_wrapper_with_backup_restores_once_and_never_moves_target_again() {
    let r = Rig::new();
    let vrc = vrc_bundled();
    write(&r.backup, &vrc);
    let m = r.manager();

    m.tick_once();
    assert!(r.target.is_file());
    assert!(!r.backup.exists());
    assert_eq!(read(&r.target).len(), vrc.len());

    let before = r.snapshot();
    for _ in 0..5 {
        m.tick_once();
    }
    assert_eq!(r.snapshot(), before);
    assert_eq!(read(&r.target), vrc);
    assert_eq!(
        r.out
            .logs()
            .matches("our yt-dlp wrapper is missing")
            .count(),
        1
    );
}

#[test]
fn restore_now_after_patch_leaves_only_vanilla_and_writes_the_flag() {
    let r = Rig::new();
    r.write_wrapper();
    let vrc = vrc_bundled();
    write(&r.target, &vrc);
    let m = r.manager();
    m.tick_once();
    m.tick_once();
    assert!(wrapper_identity::contains_marker(&r.target));
    assert!(r.backup.is_file());

    assert!(m.restore_now("test"));

    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), vrc);
    assert!(r.flag().is_file());
    let log = r.log_text();
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("[patch] restore (test): "))
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    assert!(
        lines[0].contains("[patch] restore (test): Restored "),
        "{}",
        lines[0]
    );
    assert!(lines[0].ends_with(" flag=written"), "{}", lines[0]);
}

#[test]
fn restore_now_logs_a_failed_flag_write_by_error_kind_only() {
    let r = Rig::new();
    r.write_wrapper();
    let vrc = vrc_bundled();
    write(&r.target, &vrc);
    fs::create_dir_all(r.flag()).unwrap_or_else(|e| panic!("{e}"));
    let m = r.manager();
    m.tick_once();
    m.tick_once();

    assert!(m.restore_now("console-close"));

    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), vrc);
    assert!(!r.flag().is_file());
    let log = r.log_text();
    let line = log
        .lines()
        .find(|l| l.contains("[patch] restore (console-close): "))
        .unwrap_or_else(|| panic!("{log}"));
    assert!(line.contains(" flag=failed ("), "{line}");
    assert!(line.ends_with(')'), "{line}");
    let scratch = r.dir.path().to_string_lossy().to_lowercase();
    assert!(!line.to_lowercase().contains(&scratch), "{line}");
    assert!(!line.contains("clean_exit"), "{line}");
}

#[test]
fn restore_now_racing_a_running_loop_never_leaves_the_wrapper() {
    let r = Rig::new();
    r.write_wrapper();
    let vrc = vrc_bundled();
    for i in 0..50u64 {
        for name in r.entries() {
            fs::remove_file(r.tools.join(name)).unwrap_or_else(|e| panic!("{e}"));
        }
        write(&r.target, &vrc);
        let m = r.manager_with(Some(r.tools.clone()), Duration::from_millis(20));
        assert!(m.start());
        std::thread::sleep(Duration::from_millis(i % 10 * 7));
        assert!(m.restore_now(&format!("race {i}")));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()], "iteration {i}");
        assert_eq!(read(&r.target), vrc, "iteration {i}");
    }
}

#[test]
fn tick_after_restore_now_does_not_reapply() {
    let r = Rig::new();
    r.write_wrapper();
    let vrc = vrc_bundled();
    write(&r.target, &vrc);
    let m = r.manager();
    m.tick_once();
    m.tick_once();
    assert!(m.restore_now("test"));
    for _ in 0..4 {
        m.tick_once();
    }
    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), vrc);
}

#[test]
fn restore_now_without_a_tools_folder_still_writes_the_flag() {
    let r = Rig::new();
    r.write_wrapper();
    let m = r.manager_with(None, Duration::from_secs(3));
    assert!(m.restore_now("patch-refused"));
    assert!(r.flag().is_file());
}

#[test]
fn restore_helper_moves_the_og_into_place() {
    let r = Rig::new();
    write(&r.target, b"PATCHED");
    write(&r.backup, b"VANILLA");
    assert!(restore_ytdlp_in_tools(
        &r.tools,
        None,
        &r.ux(),
        &Log::disabled()
    ));
    assert_eq!(read(&r.target), b"VANILLA");
    assert_eq!(r.entries().len(), 1);
}

#[cfg(windows)]
#[test]
fn restore_helper_fails_when_the_target_is_fully_locked() {
    use std::os::windows::fs::OpenOptionsExt;
    let r = Rig::new();
    write(&r.target, b"PATCHED");
    write(&r.backup, b"VANILLA");
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&r.target)
        .unwrap_or_else(|e| panic!("{e}"));
    let ok = restore_ytdlp_in_tools(&r.tools, None, &r.ux(), &Log::disabled());
    drop(lock);
    assert!(!ok);
    assert!(r.backup.is_file());
    assert!(r.out.logs().contains("restore error: "));
}

#[test]
fn restore_helper_is_not_a_failure_when_the_target_is_not_ours() {
    let r = Rig::new();
    write(&r.target, b"PATCHED");
    assert!(restore_ytdlp_in_tools(
        &r.tools,
        None,
        &r.ux(),
        &Log::disabled()
    ));
    assert_eq!(read(&r.target), b"PATCHED");
}

#[test]
fn restore_helper_fails_for_a_missing_directory() {
    let r = Rig::new();
    assert!(!restore_ytdlp_in_tools(
        &r.tools.join("does-not-exist"),
        None,
        &r.ux(),
        &Log::disabled()
    ));
}
