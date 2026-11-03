use super::*;

#[test]
fn recover_after_restore_now_keeps_the_clean_exit_flag() {
    let r = Rig::new();
    r.write_wrapper();
    let vrc = vrc_bundled();
    write(&r.target, &vrc);
    let m = r.manager();
    assert!(m.restore_now("ctrl-c"));
    m.recover_from_unclean_shutdown();
    assert!(r.flag().is_file());
    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), vrc);
}

#[test]
fn recover_with_a_clean_exit_flag_consumes_it_and_leaves_tools_alone() {
    let r = Rig::new();
    write(&r.target, &wrapper_bytes());
    fs::create_dir_all(&r.state).unwrap_or_else(|e| panic!("{e}"));
    write(&r.flag(), b"x");
    let m = r.manager();
    m.recover_from_unclean_shutdown();
    assert!(!r.flag().exists());
    assert_eq!(read(&r.target), wrapper_bytes());
}

#[test]
fn recover_restores_the_og_after_an_unclean_exit() {
    let r = Rig::new();
    let og = vrc_bundled();
    write(&r.backup, &og);
    write(&r.target, &wrapper_bytes());
    let m = r.manager();
    m.recover_from_unclean_shutdown();
    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), og);
    assert!(
        r.out
            .text()
            .contains("Recovery: previous run exited uncleanly -- restoring")
    );
}

#[test]
fn recover_clears_a_read_only_orphan_wrapper() {
    let r = Rig::new();
    write(&r.target, &wrapper_bytes());
    let mut perms = fs::metadata(&r.target)
        .unwrap_or_else(|e| panic!("{e}"))
        .permissions();
    let original_permissions = perms.clone();
    perms.set_readonly(true);
    fs::set_permissions(&r.target, perms).unwrap_or_else(|e| panic!("{e}"));
    let m = r.manager();
    m.recover_from_unclean_shutdown();
    for name in r.entries() {
        let p = r.tools.join(name);
        let _ = fs::set_permissions(&p, original_permissions.clone());
    }
    assert!(!r.target.exists());
    let mut left = r
        .entries()
        .into_iter()
        .filter(|n| !n.starts_with("yt-dlp.exe.stale-"));
    assert!(left.next().is_none(), "{:?}", r.entries());
    let scratch = r.dir.path().to_string_lossy().to_lowercase();
    assert!(!r.out.text().to_lowercase().contains(&scratch));
}

#[test]
fn recover_removes_an_orphan_wrapper_known_only_by_its_hash() {
    let r = Rig::new();
    write(&r.target, &non_pe(8192));
    let sha = wrapper_identity::sha256_hex(&r.target).unwrap_or_default();
    fs::create_dir_all(r.install.join("data")).unwrap_or_else(|e| panic!("{e}"));
    write(
        &r.install.join("data").join("wrapper_hashes.txt"),
        format!("{sha} v1\n").as_bytes(),
    );
    let m = r.manager();
    m.recover_from_unclean_shutdown();
    assert!(r.entries().is_empty(), "{:?}", r.entries());
}

#[test]
fn recover_after_a_kill_mid_restore() {
    let r = Rig::new();
    let og = vrc_bundled();
    write(&r.backup, &og);
    let mut stale = r.target.as_os_str().to_owned();
    stale.push(".stale-x");
    write(Path::new(&stale), &wrapper_bytes());
    let m = r.manager();
    m.recover_from_unclean_shutdown();
    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), og);
}
