mod common;

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::mesh::Capture;
use tempfile::TempDir;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::wrapper_identity;
use vrcr_watchdog::maintenance::patch::{
    PatchConfig, PatchManager, atomic_copy, is_target_in_use, restore_ytdlp_in_tools, vrc_process_state_line,
};

const PE_OFFSET: usize = 0x80;
const DIRECTORIES: usize = PE_OFFSET + 24 + 112;
const COOKIE_TRAILER: usize = 80;

fn bundled_len() -> usize {
    usize::try_from(wrapper_identity::OURS_SIZE_CEILING).unwrap_or(0) + 4096
}

fn non_pe(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + 1) & 0xFF) as u8).collect()
}

fn skeleton(len: usize) -> Vec<u8> {
    let mut d = non_pe(len);
    d[PE_OFFSET..DIRECTORIES + 16 * 8].fill(0);
    d[0] = b'M';
    d[1] = b'Z';
    d[0x3C..0x40].copy_from_slice(&u32::try_from(PE_OFFSET).unwrap_or(0).to_le_bytes());
    d[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(b"PE\0\0");
    d[PE_OFFSET + 24..PE_OFFSET + 26].copy_from_slice(&0x20Bu16.to_le_bytes());
    d
}

fn signed(len: usize, cert: usize) -> Vec<u8> {
    let mut d = skeleton(len);
    let start = u32::try_from(len - cert).unwrap_or(0);
    d[DIRECTORIES + 32..DIRECTORIES + 36].copy_from_slice(&start.to_le_bytes());
    d[DIRECTORIES + 36..DIRECTORIES + 40].copy_from_slice(&u32::try_from(cert).unwrap_or(0).to_le_bytes());
    d
}

fn unsigned_with_cookie(len: usize) -> Vec<u8> {
    let mut d = skeleton(len);
    let cookie = wrapper_identity::PYINSTALLER_COOKIE;
    let at = len - cookie.len() - COOKIE_TRAILER;
    d[at..at + cookie.len()].copy_from_slice(cookie);
    d[at + cookie.len()..].fill(0);
    d
}

fn cut(mut d: Vec<u8>, n: usize) -> Vec<u8> {
    d.truncate(d.len() - n);
    d
}

fn vrc_bundled() -> Vec<u8> {
    unsigned_with_cookie(bundled_len())
}

fn wrapper_bytes() -> Vec<u8> {
    let mut d: Vec<u8> = (0..8192).map(|i| ((i * 31) & 0xFF) as u8).collect();
    let marker = wrapper_identity::marker();
    d[256..256 + marker.len()].copy_from_slice(&marker);
    d
}

fn read(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
}

struct Rig {
    dir: TempDir,
    install: PathBuf,
    tools: PathBuf,
    state: PathBuf,
    wrapper: PathBuf,
    target: PathBuf,
    backup: PathBuf,
    out: Capture,
}

impl Rig {
    fn new() -> Rig {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let install = dir.path().join("install");
        let tools = dir.path().join("vrc-tools");
        let state = dir.path().join("state");
        fs::create_dir_all(install.join("tools")).unwrap_or_else(|e| panic!("{e}"));
        fs::create_dir_all(&tools).unwrap_or_else(|e| panic!("{e}"));
        Rig {
            wrapper: install.join("tools").join("yt-dlp.exe"),
            target: tools.join("yt-dlp.exe"),
            backup: tools.join("yt-dlp-og.exe"),
            install,
            tools,
            state,
            dir,
            out: Capture::default(),
        }
    }

    fn ux(&self) -> Ux {
        Ux::new(Box::new(self.out.clone()), Style::plain(), String::new)
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("watchdog.log")
    }

    fn manager_with(&self, tools: Option<PathBuf>, tick: Duration) -> Arc<PatchManager> {
        Arc::new(PatchManager::new(
            PatchConfig {
                install_dir: self.install.clone(),
                vrc_tools_dir: tools,
                state_dir: self.state.clone(),
                tick_delay: tick,
            },
            self.ux(),
            Log::to_file(&self.log_path()),
        ))
    }

    fn manager(&self) -> Arc<PatchManager> {
        self.manager_with(Some(self.tools.clone()), Duration::from_secs(3))
    }

    fn write_wrapper(&self) {
        write(&self.wrapper, &wrapper_bytes());
    }

    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.tools)
            .unwrap_or_else(|e| panic!("{e}"))
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn flag(&self) -> PathBuf {
        self.state.join("clean_exit.flag")
    }

    fn log_text(&self) -> String {
        fs::read_to_string(self.log_path()).unwrap_or_default()
    }

    fn snapshot(&self) -> Vec<(String, u64, SystemTime)> {
        let mut v: Vec<(String, u64, SystemTime)> = fs::read_dir(&self.tools)
            .unwrap_or_else(|e| panic!("{e}"))
            .flatten()
            .map(|e| {
                let m = e.metadata().unwrap_or_else(|e| panic!("{e}"));
                (
                    e.file_name().to_string_lossy().into_owned(),
                    m.len(),
                    m.modified().unwrap_or(UNIX_EPOCH),
                )
            })
            .collect();
        v.sort();
        v
    }
}

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
    assert_eq!(r.out.text().matches("our yt-dlp wrapper is missing").count(), 1);
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

#[cfg(windows)]
fn ping_padded_past_the_ceiling() -> Vec<u8> {
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let ping = read(&root.join("System32").join("PING.EXE"));
    let mut content = vec![0u8; bundled_len()];
    content[..ping.len()].copy_from_slice(&ping);
    let cookie = wrapper_identity::PYINSTALLER_COOKIE;
    let at = content.len() - cookie.len() - COOKIE_TRAILER;
    content[at..at + cookie.len()].copy_from_slice(cookie);
    content
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
    let leftovers: Vec<String> = r.entries().into_iter().filter(|n| n.contains(".new-")).collect();
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
    perms.set_readonly(true);
    fs::set_permissions(&r.target, perms).unwrap_or_else(|e| panic!("{e}"));
    let m = r.manager();
    m.recover_from_unclean_shutdown();
    for name in r.entries() {
        let p = r.tools.join(name);
        let mut perms = fs::metadata(&p).unwrap_or_else(|e| panic!("{e}")).permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = fs::set_permissions(&p, perms);
    }
    assert!(!r.target.exists());
    let left: Vec<String> = r
        .entries()
        .into_iter()
        .filter(|n| !n.starts_with("yt-dlp.exe.stale-"))
        .collect();
    assert!(left.is_empty(), "{:?}", r.entries());
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
    assert!(r.out.text().contains("hasn't shipped its own yt-dlp.exe yet"));

    write(&r.target, &vrc_bundled());
    fs::create_dir_all(&r.state).unwrap_or_else(|e| panic!("{e}"));
    write(&r.state.join("halt.flag"), b"x");
    assert!(m.start());
    assert!(m.start());
    assert!(!r.state.join("halt.flag").exists());
    assert!(m.restore_now("test"));
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
            .text()
            .matches("not classified as ours or VRChat-bundled")
            .count(),
        1
    );
}

#[test]
fn our_wrapper_without_a_backup_is_removed() {
    let r = Rig::new();
    r.write_wrapper();
    write(&r.target, &wrapper_bytes());
    let m = r.manager();
    m.tick_once();
    assert!(!r.target.exists());
    assert!(r.out.text().contains("removed the wrapper so VRChat redownloads"));
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
fn restore_helper_moves_the_og_into_place() {
    let r = Rig::new();
    write(&r.target, b"PATCHED");
    write(&r.backup, b"VANILLA");
    assert!(restore_ytdlp_in_tools(&r.tools, None, &r.ux(), &Log::disabled()));
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
    assert!(r.out.text().contains("restore error: "));
}

#[test]
fn restore_helper_is_not_a_failure_when_the_target_is_not_ours() {
    let r = Rig::new();
    write(&r.target, b"PATCHED");
    assert!(restore_ytdlp_in_tools(&r.tools, None, &r.ux(), &Log::disabled()));
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

#[test]
fn target_in_use_is_false_when_idle_or_missing() {
    let r = Rig::new();
    write(&r.target, b"stub");
    assert!(!is_target_in_use(&r.target));
    assert!(!is_target_in_use(&r.tools.join("does-not-exist.exe")));
    assert!(!is_target_in_use(&r.tools.join("no-such-dir").join("yt-dlp.exe")));
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
