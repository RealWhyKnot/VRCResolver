use super::*;

#[test]
fn repair_reapplies_the_wrapper_without_waiting_for_the_ticker() {
    let rig = Rig::new();
    rig.write_wrapper();
    let original = vrc_bundled();
    write(&rig.target, &original);
    let manager = rig.manager();
    assert!(manager.wrapper_missing());
    assert_eq!(manager.repair_wrapper(), Some(true));
    assert_eq!(read(&rig.target), wrapper_bytes());
    assert_eq!(read(&rig.backup), original);
    assert!(!manager.wrapper_missing());
    assert_eq!(manager.repair_wrapper(), Some(false));
    assert!(manager.restore_now("console-close"));
    assert_eq!(manager.repair_wrapper(), None);
    assert_eq!(read(&rig.target), original);
}

#[test]
fn wrapper_returning_after_missing_episode_resumes_backup_and_install() {
    let r = Rig::new();
    let vrc = vrc_bundled();
    write(&r.backup, &vrc);
    let m = r.manager();
    for _ in 0..3 {
        m.tick_once();
    }
    assert_eq!(read(&r.target), vrc);
    assert!(!r.backup.exists());

    r.write_wrapper();
    for _ in 0..3 {
        m.tick_once();
    }
    assert_eq!(read(&r.backup), vrc);
    assert_eq!(read(&r.target), wrapper_bytes());
}

#[test]
fn unknown_target_is_never_mutated() {
    let r = Rig::new();
    r.write_wrapper();
    let small = non_pe(4096);
    write(&r.target, &small);
    let m = r.manager();
    for _ in 0..3 {
        m.tick_once();
    }
    assert_eq!(read(&r.target), small);
    assert!(!r.backup.exists());
    assert_eq!(
        r.out
            .logs()
            .matches("not classified as ours or VRChat-bundled")
            .count(),
        1
    );
}

#[test]
fn partial_vrchat_update_waits_quietly_then_reapplies() {
    let r = Rig::new();
    r.write_wrapper();
    let vrc = vrc_bundled();
    write(&r.backup, &vrc);
    write(&r.target, &wrapper_bytes());
    let m = r.manager();
    m.tick_once();
    write(&r.target, &vrc[..1024 * 1024]);
    m.tick_once();
    write(&r.target, &vrc);
    m.tick_once();
    m.tick_once();
    assert_eq!(read(&r.target), wrapper_bytes());
    assert_eq!(read(&r.backup), vrc);
    let logs = r.out.logs();
    assert_eq!(logs.matches("neither our wrapper").count(), 0);
    assert_eq!(logs.matches("wrapper re-applied").count(), 1);
    assert_eq!(r.log_text().matches("still being written").count(), 1);
}

#[test]
fn settled_foreign_target_is_reported_once() {
    let r = Rig::new();
    r.write_wrapper();
    write(&r.backup, &vrc_bundled());
    let foreign = non_pe(4096);
    write(&r.target, &foreign);
    let m = r.manager();
    for _ in 0..4 {
        m.tick_once();
    }
    assert_eq!(read(&r.target), foreign);
    assert_eq!(r.out.logs().matches("neither our wrapper").count(), 1);
}

#[test]
fn target_in_use_is_false_when_idle_or_missing() {
    let r = Rig::new();
    write(&r.target, b"stub");
    assert!(!is_target_in_use(&r.target));
    assert!(!is_target_in_use(&r.tools.join("does-not-exist.exe")));
    assert!(!is_target_in_use(
        &r.tools.join("no-such-dir").join("yt-dlp.exe")
    ));
}

#[cfg(windows)]
#[test]
fn target_in_use_is_true_for_any_open_handle() {
    use std::os::windows::fs::OpenOptionsExt;
    let r = Rig::new();
    write(&r.target, b"stub");
    for (share, write_access) in [(0u32, false), (1, false), (3, true)] {
        let holder = OpenOptions::new()
            .read(true)
            .write(write_access)
            .share_mode(share)
            .open(&r.target)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(is_target_in_use(&r.target), "share={share}");
        drop(holder);
    }
    let _ = File::open(&r.target);
    assert!(!is_target_in_use(&r.target));
}
