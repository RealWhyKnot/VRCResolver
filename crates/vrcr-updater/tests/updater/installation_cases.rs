use super::*;

#[test]
fn relative_paths_are_normalized_and_escapes_refused() {
    assert_eq!(
        normalize_rel(r"tools\yt-dlp.exe"),
        Some(PathBuf::from("tools").join("yt-dlp.exe"))
    );
    assert_eq!(
        normalize_rel(" data//a.txt "),
        Some(PathBuf::from("data").join("a.txt"))
    );
    for bad in ["", "/abs", r"C:\x", "a/../b", "./a", "..", "   "] {
        assert_eq!(normalize_rel(bad), None, "{bad:?}");
    }
    let root = Path::new("install");
    assert_eq!(
        safe_install_path(root, "a/b.txt"),
        Ok(root.join("a").join("b.txt"))
    );
    assert_eq!(
        safe_install_path(root, "../x").err().as_deref(),
        Some("Unsafe release manifest path: ../x")
    );
}

#[test]
fn manifest_paths_skip_comments_and_bad_rows() {
    let dir = tmp();
    write(
        &dir.path().join("data/release-manifest.tsv"),
        &format!(
            "# header\n{SHA_A}\t1\tvrcresolver.exe\n{SHA_A}\t2\tTools/YT-DLP.exe\nbroken line\n{SHA_A}\t3\t../escape\n\n"
        ),
    );
    let paths = read_manifest_paths(dir.path());
    let keys: Vec<&String> = paths.keys().collect();
    assert_eq!(keys, vec!["tools/yt-dlp.exe", "vrcresolver.exe"]);
    assert_eq!(
        paths.get("tools/yt-dlp.exe").map(String::as_str),
        Some("Tools/YT-DLP.exe")
    );
    assert!(read_manifest_paths(&dir.path().join("missing")).is_empty());
}

#[test]
fn payload_root_is_the_zip_root_or_its_single_folder() {
    let dir = tmp();
    write(&dir.path().join("vrcresolver.exe"), "x");
    assert_eq!(
        resolve_payload_root(dir.path()),
        Ok(dir.path().to_path_buf())
    );

    let nested = tmp();
    write(&nested.path().join("pkg/WKVRCProxy.exe"), "x");
    write(&nested.path().join("other/readme.txt"), "x");
    assert_eq!(
        resolve_payload_root(nested.path()),
        Ok(nested.path().join("pkg"))
    );

    let ambiguous = tmp();
    write(&ambiguous.path().join("a/vrcresolver.exe"), "x");
    write(&ambiguous.path().join("b/vrcresolver.exe"), "x");
    let err = resolve_payload_root(ambiguous.path())
        .err()
        .unwrap_or_default();
    assert!(err.contains("recognizable payload root"), "{err}");
}

#[test]
fn staged_updater_copy_is_made_only_when_the_updater_ships() {
    let dir = tmp();
    assert_eq!(ensure_staged_updater_copy(dir.path()), Ok(()));
    assert_eq!(names(dir.path()), <[std::string::String; 0]>::default());
    write(&dir.path().join("vrcresolver.Updater.exe"), "new updater");
    assert_eq!(ensure_staged_updater_copy(dir.path()), Ok(()));
    assert_eq!(
        read(&dir.path().join("vrcresolver.Updater.next.exe")),
        "new updater"
    );
}

#[test]
fn copy_over_replaces_files_skips_the_updater_and_drops_removed_ones() {
    let from = tmp();
    let to = tmp();
    write(&from.path().join("vrcresolver.exe"), "new watchdog");
    write(&from.path().join("vrcresolver.Updater.exe"), "new updater");
    write(&from.path().join("tools/yt-dlp.exe"), "new shim");
    write(
        &from.path().join("data/release-manifest.tsv"),
        &manifest(&[
            "vrcresolver.exe",
            "vrcresolver.Updater.exe",
            "tools/yt-dlp.exe",
        ]),
    );
    write(&to.path().join("vrcresolver.exe"), "old watchdog");
    write(
        &to.path().join("vrcresolver.Updater.exe"),
        "running updater",
    );
    write(&to.path().join("tools/yt-dlp.exe"), "old shim");
    write(&to.path().join("tools/retired.dll"), "retired");
    write(&to.path().join("user-notes.txt"), "keep me");
    write(
        &to.path().join("data/release-manifest.tsv"),
        &manifest(&["vrcresolver.exe", "tools/yt-dlp.exe", "TOOLS/retired.dll"]),
    );

    atomic_copy_over(from.path(), to.path()).unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(read(&to.path().join("vrcresolver.exe")), "new watchdog");
    assert_eq!(
        read(&to.path().join("vrcresolver.Updater.exe")),
        "running updater"
    );
    assert_eq!(read(&to.path().join("tools/yt-dlp.exe")), "new shim");
    assert_eq!(read(&to.path().join("user-notes.txt")), "keep me");
    assert!(!to.path().join("tools/retired.dll").exists());
    assert_eq!(names(&to.path().join("tools")), vec!["yt-dlp.exe"]);
    assert_eq!(
        names(to.path()),
        vec![
            "data",
            "tools",
            "user-notes.txt",
            "vrcresolver.Updater.exe",
            "vrcresolver.exe"
        ]
    );
}

#[test]
fn copy_over_without_an_old_manifest_deletes_nothing() {
    let from = tmp();
    let to = tmp();
    write(&from.path().join("vrcresolver.exe"), "new");
    write(
        &from.path().join("data/release-manifest.tsv"),
        &manifest(&["vrcresolver.exe"]),
    );
    write(&to.path().join("extra.dll"), "kept");
    atomic_copy_over(from.path(), to.path()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(read(&to.path().join("extra.dll")), "kept");
}

#[test]
fn copy_over_rolls_back_every_swapped_file_when_one_cannot_be_replaced() {
    let from = tmp();
    let to = tmp();
    write(&from.path().join("a.txt"), "new a");
    write(&from.path().join("blocked"), "new file where a folder sits");
    write(&from.path().join("c.txt"), "new c");
    write(&to.path().join("a.txt"), "old a");
    write(&to.path().join("blocked/inner.txt"), "folder content");
    write(&to.path().join("c.txt"), "old c");

    let err = atomic_copy_over(from.path(), to.path())
        .err()
        .unwrap_or_default();
    assert!(err.contains("Failed to replace"), "{err}");
    assert_eq!(read(&to.path().join("a.txt")), "old a");
    assert_eq!(read(&to.path().join("c.txt")), "old c");
    assert_eq!(read(&to.path().join("blocked/inner.txt")), "folder content");
    assert_eq!(names(to.path()), vec!["a.txt", "blocked", "c.txt"]);
}

#[cfg(windows)]
#[test]
fn copy_over_names_a_locked_yt_dlp_and_leaves_the_install_untouched() {
    use std::os::windows::fs::OpenOptionsExt;
    let from = tmp();
    let to = tmp();
    write(&from.path().join("tools/yt-dlp.exe"), "new shim");
    write(&from.path().join("vrcresolver.exe"), "new watchdog");
    write(&to.path().join("tools/yt-dlp.exe"), "old shim");
    write(&to.path().join("vrcresolver.exe"), "old watchdog");
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(to.path().join("tools/yt-dlp.exe"))
        .unwrap_or_else(|e| panic!("{e}"));

    let err = atomic_copy_over(from.path(), to.path())
        .err()
        .unwrap_or_default();
    drop(lock);
    assert!(err.contains("file is locked. Close VRChat"), "{err}");
    assert_eq!(read(&to.path().join("tools/yt-dlp.exe")), "old shim");
    assert_eq!(read(&to.path().join("vrcresolver.exe")), "old watchdog");
    assert_eq!(names(&to.path().join("tools")), vec!["yt-dlp.exe"]);
    assert_eq!(names(to.path()), vec!["tools", "vrcresolver.exe"]);
}
