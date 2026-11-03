use super::*;

#[test]
fn second_store_id_is_tried_only_after_the_first_fails() {
    let installed = Arc::new(AtomicBool::new(false));
    let mut env = Env::new();
    let seen_installed = std::sync::Arc::clone(&installed);
    env.appx = Box::new(move |_| seen_installed.load(Ordering::SeqCst));
    let flip = installed;
    env.winget = Box::new(move |id| {
        if id == "free-id" {
            flip.store(true, Ordering::SeqCst);
            WingetResult::exited(0, "")
        } else {
            WingetResult::exited(1, "This app requires purchase before install.")
        }
    });
    let (_, seen) = run_codecs(&env, &mut CodecState::default());
    assert_eq!(*lock(&env.winget_calls), vec!["paid-id", "free-id"]);
    assert!(seen[0].installed);
    assert_eq!(seen[0].detail, "winget+appx");
}

#[cfg(windows)]
#[test]
fn media_foundation_probe_answers_on_this_machine() {
    let all = codec::probe_all();
    let verified = all.unwrap_or_else(|| panic!("Media Foundation did not start"));
    for c in &verified {
        assert!(wire::EXTENSION_BACKED_VIDEO_CODECS.contains(&c.as_str()));
        assert_eq!(codec::probe_decoder(c), Some(true));
    }
}

#[cfg(windows)]
#[test]
fn parent_process_names_whoever_started_the_test_binary() {
    let parent = vrcr_watchdog::maintenance::describe_parent_process()
        .unwrap_or_else(|| panic!("no parent"));
    let (name, pid) = parent
        .split_once(" pid=")
        .unwrap_or_else(|| panic!("{parent}"));
    assert!(
        !name.is_empty() && !name.contains(' ') && !name.to_lowercase().ends_with(".exe"),
        "{parent}"
    );
    let pid: u32 = pid.parse().unwrap_or_else(|e| panic!("{parent}: {e}"));
    assert_ne!(pid, std::process::id());
    let procs = vrcr_shared::win::list_processes();
    let found = procs
        .iter()
        .find(|p| p.pid == pid)
        .unwrap_or_else(|| panic!("{parent}"));
    assert_eq!(vrcr_shared::win::process_stem(&found.exe), name);
    let mine = vrcr_shared::win::process_start_time(std::process::id());
    let theirs = vrcr_shared::win::process_start_time(pid);
    if let (Some(mine), Some(theirs)) = (mine, theirs) {
        assert!(theirs <= mine);
    }
}

#[test]
fn describe_parent_handles_missing_and_self_parented_entries() {
    use vrcr_shared::win::{ProcessInfo, describe_parent};
    let p = |pid, parent_pid, exe: &str| ProcessInfo {
        pid,
        parent_pid,
        exe: exe.to_owned(),
    };
    let list = vec![
        p(4, 0, "System"),
        p(100, 50, "cargo.exe"),
        p(200, 100, "tests.EXE"),
        p(300, 300, "odd.exe"),
    ];
    assert_eq!(
        describe_parent(&list, 200).as_deref(),
        Some("cargo pid=100")
    );
    assert_eq!(describe_parent(&list, 100), None);
    assert_eq!(describe_parent(&list, 300), None);
    assert_eq!(describe_parent(&list, 999), None);
    assert_eq!(vrcr_shared::win::process_stem("a.exe.exe"), "a.exe");
    assert_eq!(vrcr_shared::win::process_stem(".exe"), ".exe");
    assert_eq!(vrcr_shared::win::process_stem("VRChat"), "VRChat");
}
