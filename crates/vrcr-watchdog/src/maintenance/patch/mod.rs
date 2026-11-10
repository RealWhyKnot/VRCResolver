use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use vrcr_shared::console::{Component, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::logutil::{iso8601_utc, sanitize_for_console};
use vrcr_shared::tools::{self, OG_NAME, RealOps, RestoreOutcome, TARGET_NAME};
use vrcr_shared::wrapper_identity::{self, WrapperKind};

pub const DEFAULT_TICK_DELAY: Duration = Duration::from_secs(3);
pub const MIN_REAPPLY_GAP: Duration = Duration::from_secs(3);
const PATCH_PREFIX: &str = "[patch] ";
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickOutcome {
    None,
    Match,
    Locked,
    Reapplied,
    ReapplyFailed,
    BackupCreated,
    InitialStaged,
    Waiting,
    UnknownTarget,
    BackupLost,
    WrapperMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
}

struct ClassifyCache {
    path: PathBuf,
    stamp: Stamp,
    kind: WrapperKind,
}

struct TickState {
    restored: bool,
    last_patch: Option<Instant>,
    classify: Option<ClassifyCache>,
    previous: Option<Stamp>,
    incomplete_reported: Option<Stamp>,
    last_outcome: TickOutcome,
    lock_failures: u32,
}

pub struct PatchConfig {
    pub install_dir: PathBuf,
    pub vrc_tools_dir: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub tick_delay: Duration,
}

pub struct PatchManager {
    wrapper: PathBuf,
    known_hashes: PathBuf,
    clean_exit_flag: PathBuf,
    halt_flag: PathBuf,
    tools_dir: Option<PathBuf>,
    tick_delay: Duration,
    ux: Ux,
    log: Log,
    alive: fn(u32) -> bool,
    started: AtomicBool,
    stop: Mutex<Option<mpsc::Sender<()>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    state: Mutex<TickState>,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    meta.is_file().then_some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok()?,
    })
}

fn kind_text(e: &io::Error) -> String {
    format!("{:?}", e.kind())
}

fn strip_prefix(message: &str) -> &str {
    message.strip_prefix(PATCH_PREFIX).unwrap_or(message)
}

pub fn is_target_in_use(path: &Path) -> bool {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(0);
    }
    match opts.open(path) {
        Ok(_) => false,
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
            ) =>
        {
            true
        }
        Err(e) => !matches!(
            e.kind(),
            io::ErrorKind::NotFound
                | io::ErrorKind::PermissionDenied
                | io::ErrorKind::InvalidInput
                | io::ErrorKind::InvalidFilename
                | io::ErrorKind::NotADirectory
        ),
    }
}

pub fn atomic_copy(src: &Path, dst: &Path) -> io::Result<()> {
    let mut tmp = dst.as_os_str().to_owned();
    tmp.push(format!(
        ".new-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    let tmp = PathBuf::from(tmp);
    let result = fs::copy(src, &tmp).and_then(|_| fs::rename(&tmp, dst));
    if result.is_err() && tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn restore_ytdlp_in_tools(
    tools_dir: &Path,
    known_hashes: Option<&Path>,
    ux: &Ux,
    log: &Log,
) -> bool {
    let (outcome, detail) = tools::restore(tools_dir, known_hashes, &RealOps);
    if outcome == RestoreOutcome::Failed {
        ux.warn(Component::Patch, &format!("restore error: {detail}"));
        false
    } else {
        log.line(&format!("[patch] restore: {outcome:?} -- {detail}"));
        true
    }
}

pub fn vrc_process_state_line(running: &[(u32, Option<SystemTime>)]) -> String {
    let Some(&(first_pid, _)) = running.first() else {
        return "VRChat not detected -- patch will apply immediately.".to_owned();
    };
    let primary = running
        .iter()
        .filter_map(|&(pid, t)| t.map(|t| (pid, t)))
        .min_by_key(|&(_, t)| t);
    let (pid, started) = primary.map_or((first_pid, None), |(pid, t)| (pid, Some(t)));
    let started = started.map_or_else(
        || "<unknown>".to_owned(),
        |t| {
            let full = iso8601_utc(t);
            format!("{}Z", &full[..19])
        },
    );
    format!(
        "VRChat is currently running (PID {pid}, started {started}) -- patch will apply when yt-dlp.exe isn't actively in use."
    )
}

pub fn log_vrc_process_state(ux: &Ux) {
    let running: Vec<(u32, Option<SystemTime>)> = vrcr_shared::win::list_processes()
        .into_iter()
        .filter(|p| vrcr_shared::win::process_stem(&p.exe).eq_ignore_ascii_case("VRChat"))
        .map(|p| (p.pid, vrcr_shared::win::process_start_time(p.pid)))
        .collect();
    ux.write(Component::Patch, &vrc_process_state_line(&running));
}

impl PatchManager {
    pub fn new(config: PatchConfig, ux: Ux, log: Log) -> PatchManager {
        let _ = fs::create_dir_all(&config.state_dir);
        PatchManager {
            wrapper: config.install_dir.join("tools").join(TARGET_NAME),
            known_hashes: config.install_dir.join("data").join("wrapper_hashes.txt"),
            clean_exit_flag: config.state_dir.join("clean_exit.flag"),
            halt_flag: config.state_dir.join("halt.flag"),
            tools_dir: config.vrc_tools_dir,
            tick_delay: config.tick_delay,
            ux,
            log,
            alive: vrcr_shared::win::process_alive,
            started: AtomicBool::new(false),
            stop: Mutex::new(None),
            worker: Mutex::new(None),
            state: Mutex::new(TickState {
                restored: false,
                last_patch: None,
                classify: None,
                previous: None,
                incomplete_reported: None,
                last_outcome: TickOutcome::None,
                lock_failures: 0,
            }),
        }
    }

    pub fn vrc_tools_dir(&self) -> Option<&Path> {
        self.tools_dir.as_deref()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TickState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn classify(&self, st: &mut TickState, path: &Path) -> WrapperKind {
        let stamp = stamp_of(path);
        if let (Some(stamp), Some(c)) = (stamp, st.classify.as_ref())
            && c.path == path
            && c.stamp == stamp
        {
            return c.kind;
        }
        let kind = wrapper_identity::classify(path, Some(&self.known_hashes));
        if let Some(stamp) = stamp {
            st.classify = Some(ClassifyCache {
                path: path.to_path_buf(),
                stamp,
                kind,
            });
        }
        kind
    }

    pub fn recover_from_unclean_shutdown(&self) {
        let mut st = self.lock();
        if st.restored {
            return;
        }
        if let Some(dir) = &self.tools_dir {
            tools::sweep(dir, &self.log, &self.alive);
        }
        if self.clean_exit_flag.exists() {
            let _ = fs::remove_file(&self.clean_exit_flag);
            return;
        }
        let Some(dir) = self.tools_dir.as_deref().filter(|d| d.is_dir()) else {
            return;
        };
        let target = dir.join(TARGET_NAME);
        if dir.join(OG_NAME).is_file() {
            self.ux.write(
                Component::Patch,
                "Recovery: the last run was stopped before it could clean up. Restoring VRChat's yt-dlp from yt-dlp-og.exe.",
            );
            restore_ytdlp_in_tools(dir, None, &self.ux, &self.log);
            return;
        }
        if target.is_file() && self.classify(&mut st, &target) == WrapperKind::Ours {
            self.ux.warn(
                Component::Patch,
                "Recovery: previous run exited uncleanly and left our wrapper in Tools with no backup -- removing it. VRChat will re-download its yt-dlp on next session.",
            );
            restore_ytdlp_in_tools(dir, Some(&self.known_hashes), &self.ux, &self.log);
        }
        drop(st);
    }

    pub fn start(self: &Arc<Self>) -> bool {
        if self.started.swap(true, Ordering::AcqRel) {
            return true;
        }
        let refuse = |msg: &str| {
            self.ux.error(Component::Patch, msg);
            self.started.store(false, Ordering::Release);
            false
        };
        let Some(dir) = self.tools_dir.as_deref() else {
            return refuse(
                "Cannot apply patch -- VRChat Tools folder not found. Launch VRChat once first, then re-run.",
            );
        };
        if !self.wrapper.is_file() {
            return refuse(
                "Cannot apply patch -- patched yt-dlp.exe is missing from this install. Reinstall vrcresolver.",
            );
        }
        if !dir.join(TARGET_NAME).is_file() && !dir.join(OG_NAME).is_file() {
            return refuse(
                "Cannot apply patch -- VRChat hasn't shipped its own yt-dlp.exe yet, and we have no original to preserve as fallback. Launch VRChat once first, then re-run.",
            );
        }
        if self.halt_flag.exists() {
            let _ = fs::remove_file(&self.halt_flag);
        }
        if self.lock().restored {
            return true;
        }
        let (tx, rx) = mpsc::channel::<()>();
        *self.stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        let weak: Weak<Self> = Arc::downgrade(self);
        let delay = self.tick_delay;
        let handle = thread::Builder::new()
            .name("patch-watchdog".to_owned())
            .spawn(move || {
                loop {
                    let Some(me) = weak.upgrade() else { return };
                    if me.lock().restored {
                        return;
                    }
                    me.tick_once();
                    drop(me);
                    match rx.recv_timeout(delay) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .ok();
        *self.worker.lock().unwrap_or_else(|e| e.into_inner()) = handle;
        true
    }

    fn cancel(&self) {
        drop(self.stop.lock().unwrap_or_else(|e| e.into_inner()).take());
    }

    pub fn restore_now(&self, reason: &str) -> bool {
        self.cancel();
        let mut st = self.lock();
        st.restored = true;
        let (outcome, detail) = match self.tools_dir.as_deref() {
            None => (
                RestoreOutcome::NothingToDo,
                "VRChat Tools folder not known".to_owned(),
            ),
            Some(dir) => {
                let r = tools::restore(dir, None, &RealOps);
                tools::sweep(dir, &self.log, &self.alive);
                r
            }
        };
        let line = format!("restore ({reason}): {outcome:?} {detail}");
        if outcome == RestoreOutcome::Failed {
            self.ux.warn(Component::Patch, &line);
            return false;
        }
        let flag = self.write_clean_exit_flag();
        self.log.line(&format!("[patch] {line} flag={flag}"));
        drop(st);
        true
    }

    fn write_clean_exit_flag(&self) -> String {
        match fs::write(&self.clean_exit_flag, iso8601_utc(SystemTime::now())) {
            Ok(()) => "written".to_owned(),
            Err(e) => format!("failed ({})", kind_text(&e)),
        }
    }

    pub fn tick_once(&self) {
        let mut st = self.lock();
        if st.restored {
            return;
        }
        self.tick_core(&mut st);
        drop(st);
    }
}

impl Drop for PatchManager {
    fn drop(&mut self) {
        self.cancel();
        let worker = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = worker
            && t.thread().id() != thread::current().id()
        {
            let _ = t.join();
        }
    }
}

mod patching;
