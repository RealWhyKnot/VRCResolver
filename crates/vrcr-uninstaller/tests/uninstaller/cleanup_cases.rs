use super::*;

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

#[test]
fn self_delete_only_removes_the_uninstaller_and_an_empty_folder() {
    assert_eq!(
        rmdir_log_name(&stamp()),
        "vrcresolver-uninstall-rmdir-20261004070509.log"
    );
    let dir = Path::new(r"C:\My Apps\vrcresolver");
    let exe = dir.join("vrcresolver.Uninstaller.exe");
    let args = self_delete_args(dir, &exe, Path::new(r"C:\Temp\x.log"));
    assert_eq!(
        args.as_deref(),
        Ok(
            r#"/c (ping 127.0.0.1 -n 4 > nul) & (del /f /q "C:\My Apps\vrcresolver\vrcresolver.Uninstaller.exe" & rmdir "C:\My Apps\vrcresolver") > "C:\Temp\x.log" 2>&1"#
        )
    );
    assert!(!args.unwrap_or_default().contains("/s"));
    let exe = Path::new(r"C:\a\u.exe");
    assert!(self_delete_args(Path::new("C:\\a\"b"), exe, Path::new("x")).is_err());
    assert!(self_delete_args(Path::new(r"C:\a"), exe, Path::new("x\"y")).is_err());
    assert!(
        self_delete_args(
            Path::new(r"C:\a"),
            Path::new("C:\\a\\u\".exe"),
            Path::new("x")
        )
        .is_err()
    );
    assert!(self_delete_args(Path::new(r"C:\%USERPROFILE%"), exe, Path::new("x")).is_err());
}

#[test]
fn install_files_merge_the_shipped_list_with_the_manifest() {
    let rel = |p: &str| p.split('/').collect::<PathBuf>();
    let bare = install_files(None);
    assert_eq!(bare.len(), SHIPPED_FILES.len());
    for name in SHIPPED_FILES {
        assert!(bare.contains(&rel(name)), "{name}");
    }
    let manifest = "aa\t1\tVRCResolver.exe\naa\t2\tdata/extra/new.bin\naa\t3\t../outside.txt\naa\t4\tC:\\evil.txt\nbroken\n";
    let files = install_files(Some(manifest));
    assert_eq!(files.len(), SHIPPED_FILES.len() + 1);
    assert!(files.contains(&rel("data/extra/new.bin")));
    assert!(!files.iter().any(|f| {
        let text = f.to_string_lossy();
        text.contains("outside") || text.contains("evil")
    }));
    assert_eq!(
        install_subdirs(&files),
        vec![rel("data/extra"), rel("data"), rel("tools")]
    );
}

#[test]
fn extracted_into_a_shared_folder_only_our_files_go() {
    let inst = full_install(&["data/extra.bin"]);
    let user_files = [
        "holiday.jpg",
        "Projects/report.docx",
        "data/notes.txt",
        "tools/other.exe",
    ];
    for name in user_files {
        put(&inst.path.join(name), "mine");
    }
    let f = Fake::new();
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK, "{}", f.errs());

    for name in user_files {
        assert!(inst.path.join(name).is_file(), "{name} was deleted");
    }
    for name in SHIPPED_FILES.iter().chain(&["data/extra.bin"]) {
        if *name != "vrcresolver.Uninstaller.exe" {
            assert!(!inst.path.join(name).exists(), "{name} was left behind");
        }
    }
    assert_eq!(
        entries(&inst.path),
        vec![
            "Projects",
            "data",
            "holiday.jpg",
            "tools",
            "vrcresolver.Uninstaller.exe"
        ]
    );
    assert_eq!(f.events().last(), Some(&self_delete_cmd(&inst)));
    assert!(!f.events().iter().any(|e| e.contains("rmdir /s")));
    assert!(f.out().contains("[uninstall] remove-files removed=7"));
    assert_eq!(
        f.out.borrow().last().cloned(),
        Some(format!(
            "VRCResolver uninstalled. Files it didn't install were left in {}.",
            inst.path.display()
        ))
    );
}

#[test]
fn a_folder_with_only_our_files_is_emptied_for_the_self_delete() {
    let inst = full_install(&["data/extra/deep.bin"]);
    let f = Fake::new();
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK, "{}", f.errs());
    assert_eq!(entries(&inst.path), vec!["vrcresolver.Uninstaller.exe"]);
    assert_eq!(f.events().last(), Some(&self_delete_cmd(&inst)));
    assert_eq!(
        f.out.borrow().last().map(String::as_str),
        Some("VRCResolver uninstalled. The install folder will disappear in a moment.")
    );
}

#[test]
fn manifest_paths_outside_the_install_folder_are_never_touched() {
    let inst = full_install(&[]);
    let outside = inst
        .path
        .parent()
        .map(|p| p.join("outside.txt"))
        .unwrap_or_default();
    put(&outside, "mine");
    put(
        &inst.path.join("data/release-manifest.tsv"),
        "AAAA\t1\t../outside.txt\nAAAA\t1\tvrcresolver.exe\n",
    );
    let f = Fake::new();
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_OK, "{}", f.errs());
    assert!(outside.is_file());
    assert_eq!(entries(&inst.path), vec!["vrcresolver.Uninstaller.exe"]);
}

#[test]
fn a_file_we_cannot_remove_is_reported_and_the_folder_stays() {
    let inst = full_install(&[]);
    let mut f = Fake::new();
    f.locked = HashSet::from([inst.path.join("tools").join("yt-dlp.exe")]);
    assert_eq!(flow::run(&f, &inst.exe, &roots()), EXIT_ERRORS);
    let errs = f.errs();
    assert!(errs.contains("[uninstall] could not remove"), "{errs}");
    assert!(
        errs.contains("[uninstall] remove-files ERROR 1 file(s) could not be removed"),
        "{errs}"
    );
    assert!(inst.path.join("tools").join("yt-dlp.exe").is_file());
    assert_eq!(
        entries(&inst.path),
        vec!["tools", "vrcresolver.Uninstaller.exe"]
    );
    assert_eq!(f.events().last(), Some(&self_delete_cmd(&inst)));
}
