use super::*;

#[test]
fn tick_does_not_back_up_a_truncated_signed_binary() {
    let r = Rig::new();
    r.write_wrapper();
    let truncated = cut(signed(bundled_len(), 2048), 100);
    write(&r.target, &truncated);
    let m = r.manager();
    m.tick_once();
    m.tick_once();
    assert!(!r.backup.exists());
    assert_eq!(read(&r.target), truncated);
    assert!(
        r.out
            .text()
            .contains("does not look like a complete download yet")
    );
}

#[test]
fn tick_waits_while_the_target_is_still_growing() {
    let r = Rig::new();
    r.write_wrapper();
    let mib = 1024 * 1024;
    let bundled = signed(12 * mib, 2048);
    let m = r.manager();
    let mut written = 0;
    for up_to in [4 * mib, 11 * mib, 12 * mib] {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&r.target)
            .unwrap_or_else(|e| panic!("{e}"));
        f.write_all(&bundled[written..up_to])
            .unwrap_or_else(|e| panic!("{e}"));
        drop(f);
        written = up_to;
        m.tick_once();
        assert!(!r.backup.exists());
    }
    m.tick_once();
    assert_eq!(read(&r.backup), bundled);
}

#[cfg(windows)]
#[test]
fn tick_reapply_with_a_running_target_defers_quietly() {
    use std::os::windows::process::CommandExt;
    let r = Rig::new();
    r.write_wrapper();
    write(&r.backup, &vrc_bundled());
    let ping = ping_padded_past_the_ceiling();
    write(&r.target, &ping);
    let m = r.manager();
    let mut child = std::process::Command::new(&r.target)
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    for _ in 0..3 {
        m.tick_once();
    }
    let still_running = child.try_wait().unwrap_or_else(|e| panic!("{e}")).is_none();
    let target_now = read(&r.target);
    let leftovers: Vec<String> = r
        .entries()
        .into_iter()
        .filter(|n| n.contains(".new-"))
        .collect();
    let _ = child.kill();
    let _ = child.wait();
    assert!(still_running);
    assert_eq!(target_now, ping);
    assert!(leftovers.is_empty(), "{leftovers:?}");

    for _ in 0..2 {
        if wrapper_identity::contains_marker(&r.target) {
            break;
        }
        m.tick_once();
    }
    assert_eq!(read(&r.target), wrapper_bytes());
    assert_eq!(read(&r.backup), ping);
}

#[test]
fn start_refuses_without_a_tools_folder_wrapper_or_vrchat_binary() {
    let r = Rig::new();
    let none = r.manager_with(None, Duration::from_secs(3));
    assert!(!none.start());
    assert!(r.out.text().contains("VRChat Tools folder not found"));

    let m = r.manager();
    assert!(!m.start());
    assert!(r.out.text().contains("patched yt-dlp.exe is missing"));

    r.write_wrapper();
    assert!(!m.start());
    assert!(
        r.out
            .text()
            .contains("hasn't shipped its own yt-dlp.exe yet")
    );

    write(&r.target, &vrc_bundled());
    fs::create_dir_all(&r.state).unwrap_or_else(|e| panic!("{e}"));
    write(&r.state.join("halt.flag"), b"x");
    assert!(m.start());
    assert!(m.start());
    assert!(!r.state.join("halt.flag").exists());
    assert!(m.restore_now("test"));
}

#[test]
fn our_wrapper_without_a_backup_is_removed() {
    let r = Rig::new();
    r.write_wrapper();
    write(&r.target, &wrapper_bytes());
    let m = r.manager();
    m.tick_once();
    assert!(!r.target.exists());
    assert!(
        r.out
            .text()
            .contains("removed the wrapper so VRChat redownloads")
    );
}

#[test]
fn waiting_state_is_reported_once() {
    let r = Rig::new();
    r.write_wrapper();
    let m = r.manager();
    for _ in 0..4 {
        m.tick_once();
    }
    assert_eq!(
        r.out
            .text()
            .matches("VRChat has not installed yt-dlp yet")
            .count(),
        1
    );
}

#[test]
fn vrc_process_state_names_the_oldest_process() {
    assert_eq!(
        vrc_process_state_line(&[]),
        "VRChat not detected -- patch will apply immediately."
    );
    let t = |s: u64| Some(UNIX_EPOCH + Duration::from_secs(s));
    assert_eq!(
        vrc_process_state_line(&[(10, t(1_790_704_900)), (7, t(1_790_704_800)), (12, None)]),
        "VRChat is currently running (PID 7, started 2026-09-29T18:00:00Z) -- patch will apply when yt-dlp.exe isn't actively in use."
    );
    assert_eq!(
        vrc_process_state_line(&[(42, None)]),
        "VRChat is currently running (PID 42, started <unknown>) -- patch will apply when yt-dlp.exe isn't actively in use."
    );
}
