use super::*;

#[test]
fn tick_backs_up_only_after_the_size_settles() {
    let r = Rig::new();
    r.write_wrapper();
    let bundled = signed(bundled_len(), 2048);
    write(&r.target, &bundled);
    let m = r.manager();

    m.tick_once();
    assert!(!r.backup.exists());
    assert_eq!(read(&r.target), bundled);

    m.tick_once();
    assert_eq!(read(&r.backup), bundled);
    assert_eq!(read(&r.target), wrapper_bytes());
}

#[test]
fn tick_refreshes_the_backup_only_from_a_settled_complete_replacement() {
    let r = Rig::new();
    r.write_wrapper();
    let wrapper = wrapper_bytes();
    let old_og = signed(bundled_len(), 1024);
    write(&r.backup, &old_og);
    write(&r.target, &wrapper);
    let m = r.manager();
    m.tick_once();
    m.tick_once();
    assert_eq!(read(&r.target), wrapper);

    let replacement = signed(bundled_len(), 4096);
    let partial = cut(replacement.clone(), 100);
    write(&r.target, &partial);
    m.tick_once();
    m.tick_once();
    assert_eq!(read(&r.target), partial);
    assert_eq!(read(&r.backup), old_og);

    write(&r.target, &replacement);
    m.tick_once();
    assert_eq!(read(&r.target), replacement);
    assert_eq!(read(&r.backup), old_og);

    m.tick_once();
    assert_eq!(read(&r.backup), replacement);
    assert_eq!(read(&r.target), wrapper);
}

#[test]
fn atomic_copy_replaces_the_destination_and_leaves_no_temp() {
    let r = Rig::new();
    let src = r.tools.join("source.bin");
    let dst = r.tools.join("dest.bin");
    write(&src, b"NEW");
    write(&dst, b"OLD");
    atomic_copy(&src, &dst).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(read(&dst), b"NEW");
    assert!(!r.entries().iter().any(|n| n.starts_with("dest.bin.new-")));
}

#[test]
fn atomic_copy_cleans_up_on_failure() {
    let r = Rig::new();
    let dst = r.tools.join("dest.bin");
    write(&dst, b"OLD");
    assert!(atomic_copy(&r.tools.join("missing.bin"), &dst).is_err());
    assert_eq!(read(&dst), b"OLD");
    assert!(!r.entries().iter().any(|n| n.starts_with("dest.bin.new-")));
}
