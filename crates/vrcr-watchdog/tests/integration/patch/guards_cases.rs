use super::*;

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
