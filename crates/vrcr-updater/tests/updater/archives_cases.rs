use super::*;

#[test]
fn zip_extraction_restores_the_tree() {
    let dir = tmp();
    let zip = dir.path().join("r.zip");
    build_zip(
        &zip,
        &[("pkg/vrcresolver.exe", "w"), ("pkg/tools/yt-dlp.exe", "y")],
    );
    let out = dir.path().join("out");
    extract_zip(&zip, &out).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(read(&out.join("pkg/tools/yt-dlp.exe")), "y");
    assert_eq!(resolve_payload_root(&out), Ok(out.join("pkg")));
}

#[test]
fn zip_extraction_never_writes_outside_the_target() {
    let dir = tmp();
    let zip = dir.path().join("evil.zip");
    build_zip(&zip, &[("../escaped.txt", "x"), ("ok.txt", "y")]);
    let out = dir.path().join("out");
    let _ = extract_zip(&zip, &out);
    assert!(!dir.path().join("escaped.txt").exists());
    assert!(
        !dir.path()
            .parent()
            .map(|p| p.join("escaped.txt").exists())
            .unwrap_or(false)
    );
}

#[test]
fn unreadable_zip_is_an_error() {
    let dir = tmp();
    let zip = dir.path().join("bad.zip");
    write(&zip, "not a zip");
    let err = extract_zip(&zip, &dir.path().join("out"))
        .err()
        .unwrap_or_default();
    assert!(err.starts_with("release zip is unreadable"), "{err}");
}

#[test]
fn free_space_needs_one_and_a_half_times_the_download() {
    let mib = 1024 * 1024;
    assert_eq!(check_free_space(100 * mib, Some(150 * mib), "C:\\"), Ok(()));
    assert_eq!(check_free_space(100 * mib, None, "C:\\"), Ok(()));
    assert_eq!(
        check_free_space(100 * mib, Some(149 * mib), "D:\\")
            .err()
            .as_deref(),
        Some("Insufficient free space on D:\\ for download (need ~150 MiB, have 149 MiB).")
    );
}

#[test]
fn temp_sweep_removes_only_stale_updater_artifacts() {
    let dir = tmp();
    for d in [
        "vrcresolver-extract-abc",
        "WKVRCProxy-extract-1",
        "vrcresolver-cache",
        "other-extract-1",
    ] {
        write(&dir.path().join(d).join("f.txt"), "x");
    }
    for f in [
        "vrcresolver-v1.zip",
        "WKVRCProxy-v1.zip",
        "vrcresolver-.zip",
        "notes.zip",
        "vrcresolver-v1.txt",
    ] {
        write(&dir.path().join(f), "x");
    }
    assert_eq!(temp::sweep(dir.path(), SystemTime::now()), 0);
    let later = SystemTime::now() + Duration::from_secs(2 * 24 * 3600);
    assert_eq!(temp::sweep(dir.path(), later), 5);
    assert_eq!(
        names(dir.path()),
        vec![
            "notes.zip",
            "other-extract-1",
            "vrcresolver-cache",
            "vrcresolver-v1.txt"
        ]
    );
}
