mod fixtures;

use std::cell::Cell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use vrcr_shared::log::Log;
use vrcr_shared::tools::{self, RealOps, RestoreOps, RestoreOutcome};
use vrcr_shared::wrapper_identity;

struct Dir(TempDir);

impl Dir {
    fn new() -> Dir {
        Dir(TempDir::new().unwrap_or_else(|e| panic!("{e}")))
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn target(&self) -> PathBuf {
        self.path().join("yt-dlp.exe")
    }
    fn og(&self) -> PathBuf {
        self.path().join("yt-dlp-og.exe")
    }
    fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.path().join(name);
        fs::write(&p, bytes).unwrap_or_else(|e| panic!("{e}"));
        p
    }
    fn files(&self) -> Vec<String> {
        self.entries(|p| p.is_file())
    }
    fn dirs(&self) -> Vec<String> {
        self.entries(|p| p.is_dir())
    }
    fn entries(&self, keep: impl Fn(&Path) -> bool) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(self.path())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| keep(p))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        out.sort();
        out
    }
    fn stales(&self) -> usize {
        self.files().iter().filter(|n| n.contains(".stale-")).count()
    }
}

fn read(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap_or_else(|e| panic!("{e}"))
}

struct FakeOps<'a> {
    calls: Cell<u32>,
    on_move: &'a dyn Fn(u32, &Path, &Path, bool) -> io::Result<()>,
    unhashable: Option<&'a str>,
}

impl<'a> FakeOps<'a> {
    fn moving(on_move: &'a dyn Fn(u32, &Path, &Path, bool) -> io::Result<()>) -> FakeOps<'a> {
        FakeOps {
            calls: Cell::new(0),
            on_move,
            unhashable: None,
        }
    }
}

impl RestoreOps for FakeOps<'_> {
    fn move_file(&self, from: &Path, to: &Path, overwrite: bool) -> io::Result<()> {
        self.calls.set(self.calls.get() + 1);
        (self.on_move)(self.calls.get(), from, to, overwrite)
    }
    fn hash(&self, path: &Path) -> Option<String> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        if name.as_deref() == self.unhashable {
            return None;
        }
        wrapper_identity::sha256_hex(path)
    }
}

fn real(from: &Path, to: &Path, overwrite: bool) -> io::Result<()> {
    tools::move_file(from, to, overwrite)
}

fn restore(d: &Dir, ops: &dyn RestoreOps) -> (RestoreOutcome, String) {
    tools::restore(d.path(), None, ops)
}

#[cfg(windows)]
#[test]
fn restore_moves_a_running_wrapper_aside() {
    let d = Dir::new();
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    fs::copy(Path::new(&system).join(r"System32\PING.EXE"), d.target()).unwrap_or_else(|e| panic!("{e}"));
    let og = fixtures::bundled(1);
    d.put("yt-dlp-og.exe", &og);
    let mut child = std::process::Command::new(d.target())
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));

    let (outcome, detail) = restore(&d, &RealOps);
    let still_running = child.try_wait().ok().flatten().is_none();
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(outcome, RestoreOutcome::Restored, "{detail}");
    assert!(!detail.is_empty());
    assert_eq!(read(&d.target()), og);
    assert!(!d.og().exists());
    assert_eq!(d.stales(), 1);
    assert!(still_running);
}

#[test]
fn restore_keeps_a_newer_vrchat_binary() {
    let d = Dir::new();
    let newer = fixtures::complete_bundled(1);
    d.put("yt-dlp.exe", &newer);
    d.put("yt-dlp-og.exe", &fixtures::complete_bundled(2));

    let (outcome, detail) = restore(&d, &RealOps);

    assert_eq!(outcome, RestoreOutcome::KeptNewerVrchatBinary);
    assert!(!detail.is_empty());
    assert_eq!(read(&d.target()), newer);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
}

#[test]
fn restore_moves_the_og_over_a_vrchat_binary_that_is_not_complete() {
    let d = Dir::new();
    d.put("yt-dlp.exe", &fixtures::cut(fixtures::complete_bundled(1), 100));
    let og = fixtures::complete_bundled(2);
    d.put("yt-dlp-og.exe", &og);

    let (outcome, _) = restore(&d, &RealOps);

    assert_eq!(outcome, RestoreOutcome::Restored);
    assert_eq!(read(&d.target()), og);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
}

#[test]
fn restore_moves_the_og_over_a_vrchat_binary_when_a_hash_fails() {
    for unhashable in ["yt-dlp.exe", "yt-dlp-og.exe"] {
        let d = Dir::new();
        d.put("yt-dlp.exe", &fixtures::complete_bundled(1));
        let og = fixtures::complete_bundled(2);
        d.put("yt-dlp-og.exe", &og);
        let ops = FakeOps {
            calls: Cell::new(0),
            on_move: &|_, f, t, o| real(f, t, o),
            unhashable: Some(unhashable),
        };

        let (outcome, _) = restore(&d, &ops);

        assert_eq!(outcome, RestoreOutcome::Restored, "{unhashable}");
        assert_eq!(read(&d.target()), og);
        assert_eq!(d.files(), ["yt-dlp.exe"]);
    }
}

#[test]
fn restore_never_destroys_the_target_when_the_og_vanishes() {
    let d = Dir::new();
    let original = fixtures::wrapper();
    d.put("yt-dlp.exe", &original);
    d.put("yt-dlp-og.exe", &fixtures::bundled(1));
    let ops = FakeOps::moving(&|n, _, _, _| {
        Err(io::Error::from(if n == 1 {
            io::ErrorKind::PermissionDenied
        } else {
            io::ErrorKind::NotFound
        }))
    });

    let (outcome, detail) = restore(&d, &ops);

    assert_eq!(outcome, RestoreOutcome::Failed);
    assert!(!detail.is_empty());
    assert_eq!(read(&d.target()), original);
    assert_eq!(d.stales(), 0);
}

#[test]
fn restore_leaves_the_target_alone_when_the_og_is_gone_after_the_first_move_fails() {
    let d = Dir::new();
    let original = fixtures::wrapper();
    d.put("yt-dlp.exe", &original);
    d.put("yt-dlp-og.exe", &fixtures::bundled(1));
    let ops = FakeOps::moving(&|_, from, _, _| {
        let _ = fs::remove_file(from);
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    });

    let (outcome, _) = restore(&d, &ops);

    assert_eq!(outcome, RestoreOutcome::Failed);
    assert_eq!(ops.calls.get(), 1);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
    assert_eq!(read(&d.target()), original);
}

#[test]
fn restore_puts_the_target_back_when_the_og_cannot_move_in_after_the_stale_step() {
    let d = Dir::new();
    let original = fixtures::wrapper();
    d.put("yt-dlp.exe", &original);
    let og = fixtures::bundled(1);
    d.put("yt-dlp-og.exe", &og);
    let ops = FakeOps::moving(&|_, _, _, _| Err(io::Error::from(io::ErrorKind::PermissionDenied)));

    let (outcome, detail) = restore(&d, &ops);

    assert_eq!(outcome, RestoreOutcome::Failed);
    assert!(detail.contains("put back"), "{detail}");
    assert_eq!(read(&d.target()), original);
    assert_eq!(read(&d.og()), og);
    assert_eq!(d.stales(), 0);
}

#[test]
fn restore_without_og_removes_our_wrapper() {
    let d = Dir::new();
    d.put("yt-dlp.exe", &fixtures::wrapper());

    let (outcome, detail) = restore(&d, &RealOps);

    assert_eq!(outcome, RestoreOutcome::RemovedOrphanWrapper);
    assert!(!detail.is_empty());
    assert!(d.files().is_empty());
}

#[test]
fn restore_without_og_leaves_a_target_that_is_not_ours() {
    let d = Dir::new();
    let vrc = fixtures::bundled(1);
    d.put("yt-dlp.exe", &vrc);

    let (outcome, _) = restore(&d, &RealOps);

    assert_eq!(outcome, RestoreOutcome::NothingToDo);
    assert_eq!(read(&d.target()), vrc);
}

#[test]
fn restore_with_nothing_in_tools_has_nothing_to_do() {
    let d = Dir::new();
    assert_eq!(restore(&d, &RealOps).0, RestoreOutcome::NothingToDo);
    assert!(d.files().is_empty());
}

#[test]
fn restore_with_only_the_og_moves_it_into_place() {
    let d = Dir::new();
    let og = fixtures::bundled(1);
    d.put("yt-dlp-og.exe", &og);

    let (outcome, detail) = restore(&d, &RealOps);

    assert_eq!(outcome, RestoreOutcome::Restored);
    assert!(!detail.is_empty());
    assert_eq!(read(&d.target()), og);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
}

#[test]
fn restore_replaces_an_idle_wrapper_without_leaving_a_stale() {
    let d = Dir::new();
    d.put("yt-dlp.exe", &fixtures::wrapper());
    let og = fixtures::bundled(1);
    d.put("yt-dlp-og.exe", &og);

    assert_eq!(restore(&d, &RealOps).0, RestoreOutcome::Restored);
    assert_eq!(read(&d.target()), og);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
}

#[test]
fn restore_replaces_a_vrchat_binary_that_matches_the_og() {
    let d = Dir::new();
    let same = fixtures::bundled(1);
    d.put("yt-dlp.exe", &same);
    d.put("yt-dlp-og.exe", &same);

    assert_eq!(restore(&d, &RealOps).0, RestoreOutcome::Restored);
    assert_eq!(read(&d.target()), same);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
}

#[test]
fn restore_fails_for_a_missing_directory() {
    let d = Dir::new();
    let (outcome, detail) = tools::restore(&d.path().join("does-not-exist"), None, &RealOps);
    assert_eq!(outcome, RestoreOutcome::Failed);
    assert!(!detail.is_empty());
}

#[test]
fn restore_details_never_carry_a_full_path() {
    let d = Dir::new();
    d.put("yt-dlp.exe", &fixtures::wrapper());
    d.put("yt-dlp-og.exe", &fixtures::bundled(1));
    let dir = d.path().display().to_string();
    let fail = |_: u32, _: &Path, _: &Path, _: bool| Err(io::Error::other(format!("simulated at {dir}")));
    let ops = FakeOps::moving(&fail);

    let (_, detail) = restore(&d, &ops);

    assert!(!detail.contains(&dir), "{detail}");
    assert!(!detail.contains(&std::env::temp_dir().display().to_string()));
    assert!(detail.bytes().all(|b| (0x20..0x7F).contains(&b)));
}

#[test]
fn move_file_without_overwrite_refuses_an_existing_target() {
    let d = Dir::new();
    let a = d.put("a", b"a");
    let b = d.put("b", b"b");
    let err = tools::move_file(&a, &b, false).err().map(|e| e.kind());
    assert_eq!(err, Some(io::ErrorKind::AlreadyExists));
    assert_eq!(read(&b), b"b");
    tools::move_file(&a, &b, true).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(read(&b), b"a");
}

fn sweep(d: &Dir) {
    tools::sweep(d.path(), &Log::disabled(), &vrcr_shared::win::process_alive);
}

#[test]
fn sweep_deletes_known_sidecars_and_leaves_everything_else() {
    let d = Dir::new();
    for n in [
        "yt-dlp.exe.new-ab12cd34",
        "yt-dlp.exe.stale-20260503120000123",
        "yt-dlp-og.exe.new-ef56gh78",
        "yt-dlp-og.exe.stale-20260503120000456",
        "yt-dlp.exe",
        "yt-dlp-og.exe",
        "yt-dlp.exe.bak",
        "yt-dlp.exe.config",
        "yt-dlp.exe.log",
        "yt-dlp-patched.exe.new-xyz123ab",
        "ytdlp.exe.new-ab12cd34",
        "README.txt",
        "VRChat.log",
    ] {
        d.put(n, b"");
    }

    sweep(&d);

    assert_eq!(
        d.files(),
        [
            "README.txt",
            "VRChat.log",
            "yt-dlp-og.exe",
            "yt-dlp-patched.exe.new-xyz123ab",
            "yt-dlp.exe",
            "yt-dlp.exe.bak",
            "yt-dlp.exe.config",
            "yt-dlp.exe.log",
            "ytdlp.exe.new-ab12cd34",
        ]
    );
}

#[test]
fn sweep_handles_missing_directory_silently() {
    let d = Dir::new();
    tools::sweep(&d.path().join("does-not-exist"), &Log::disabled(), &|_| true);
    tools::sweep(Path::new(""), &Log::disabled(), &|_| true);
}

#[test]
fn sweep_is_case_insensitive() {
    let d = Dir::new();
    d.put("YT-DLP.EXE.NEW-ABCD1234", b"");
    d.put("yt-dlp.EXE.STALE-20260503", b"");
    sweep(&d);
    assert!(d.files().is_empty());
}

#[test]
fn sweep_deletes_legacy_yt_dlp_wrapper_log() {
    let d = Dir::new();
    d.put("yt-dlp-wrapper.log", b"");
    d.put("yt-dlp-wrapper.log.bak", b"");
    d.put("yt-dlp.exe", b"");
    sweep(&d);
    assert_eq!(d.files(), ["yt-dlp-wrapper.log.bak", "yt-dlp.exe"]);
}

#[test]
fn sweep_deletes_pyinstaller_temp_dirs_whose_owning_process_is_gone() {
    let d = Dir::new();
    let mine = format!("_MEI{}", std::process::id());
    for n in ["_MEI999999998", mine.as_str(), "_MEInotapid", "_MEI", "notes"] {
        fs::create_dir(d.path().join(n)).unwrap_or_else(|e| panic!("{e}"));
        fs::write(d.path().join(n).join("python313.dll"), b"").unwrap_or_else(|e| panic!("{e}"));
    }
    d.put("yt-dlp.exe", b"");

    sweep(&d);

    let mut expected = vec![
        "_MEI".to_owned(),
        mine,
        "_MEInotapid".to_owned(),
        "notes".to_owned(),
    ];
    expected.sort();
    assert_eq!(d.dirs(), expected);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
}

#[test]
fn pyinstaller_temp_names_parse_only_short_digit_runs() {
    assert_eq!(tools::pyinstaller_temp_pid("_MEI1234"), Some(1234));
    assert_eq!(tools::pyinstaller_temp_pid("_mei7"), Some(7));
    assert_eq!(tools::pyinstaller_temp_pid("_MEI"), None);
    assert_eq!(tools::pyinstaller_temp_pid("_MEI1234567890"), None);
    assert_eq!(tools::pyinstaller_temp_pid("_MEI12a"), None);
    assert_eq!(tools::pyinstaller_temp_pid("x_MEI12"), None);
}

#[test]
fn sweep_restores_a_bundled_stale_when_target_and_og_are_missing() {
    let d = Dir::new();
    let bundled = fixtures::bundled(1);
    d.put("yt-dlp.exe.stale-20261011120000", &bundled);
    let log_path = d
        .path()
        .join("..")
        .join(format!("sweep-{}.log", std::process::id()));
    let log = Log::to_file(&log_path);

    tools::sweep(d.path(), &log, &|_| false);
    log.close();
    let logged = fs::read_to_string(&log_path).unwrap_or_default();
    let _ = fs::remove_file(&log_path);

    assert_eq!(d.files(), ["yt-dlp.exe"]);
    assert_eq!(read(&d.target()), bundled);
    assert!(
        logged.contains("[patch] sweep: put VRChat's yt-dlp.exe back from yt-dlp.exe.stale-20261011120000")
    );
}

#[test]
fn sweep_restores_only_the_newest_bundled_stale_and_deletes_the_rest() {
    let d = Dir::new();
    d.put("yt-dlp.exe.stale-20261010090000", &fixtures::bundled(1));
    let newer = fixtures::bundled(2);
    d.put("yt-dlp.exe.stale-20261011120000", &newer);
    d.put("yt-dlp.exe.new-ab12cd34", &fixtures::wrapper());
    sweep(&d);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
    assert_eq!(read(&d.target()), newer);
}

#[test]
fn sweep_still_deletes_a_wrapper_stale() {
    let d = Dir::new();
    d.put("yt-dlp.exe.stale-20261011120000", &fixtures::wrapper());
    sweep(&d);
    assert!(d.files().is_empty());
}

#[test]
fn sweep_does_not_restore_a_bundled_stale_over_an_existing_target() {
    let d = Dir::new();
    let target = fixtures::bundled(1);
    d.put("yt-dlp.exe", &target);
    d.put("yt-dlp.exe.stale-20261011120000", &fixtures::bundled(2));
    sweep(&d);
    assert_eq!(d.files(), ["yt-dlp.exe"]);
    assert_eq!(read(&d.target()), target);
}

#[test]
fn sweep_does_not_restore_a_bundled_stale_while_the_og_exists() {
    let d = Dir::new();
    let og = fixtures::bundled(1);
    d.put("yt-dlp-og.exe", &og);
    d.put("yt-dlp.exe.stale-20261011120000", &fixtures::bundled(2));
    sweep(&d);
    assert_eq!(d.files(), ["yt-dlp-og.exe"]);
    assert_eq!(read(&d.og()), og);
}

#[test]
fn sweep_legacy_install_tools_removes_only_the_fallback_files() {
    let d = Dir::new();
    fs::create_dir(d.path().join("tools")).unwrap_or_else(|e| panic!("{e}"));
    for n in [
        "yt-dlp-og-fallback.exe",
        "yt-dlp-og-fallback.version.txt",
        "keep.txt",
    ] {
        fs::write(d.path().join("tools").join(n), b"").unwrap_or_else(|e| panic!("{e}"));
    }
    tools::sweep_legacy_install_tools(d.path());
    let left: Vec<String> = fs::read_dir(d.path().join("tools"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["keep.txt"]);
}
