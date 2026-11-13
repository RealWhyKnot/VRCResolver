use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::common::mesh::Capture;
use tempfile::TempDir;
use vrcr_shared::console::Ux;
use vrcr_shared::log::Log;
use vrcr_shared::wrapper_identity;
use vrcr_watchdog::maintenance::patch::{
    PatchConfig, PatchManager, atomic_copy, is_target_in_use, restore_ytdlp_in_tools,
    vrc_process_state_line,
};

const PE_OFFSET: usize = 0x80;
const DIRECTORIES: usize = PE_OFFSET + 24 + 112;
const COOKIE_TRAILER: usize = 80;

fn bundled_len() -> usize {
    usize::try_from(wrapper_identity::OURS_SIZE_CEILING).unwrap_or(0) + 4096
}

fn non_pe(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| u8::try_from((i * 7 + 1) & 0xFF).expect("masked fixture value fits one byte"))
        .collect()
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
    d[DIRECTORIES + 36..DIRECTORIES + 40]
        .copy_from_slice(&u32::try_from(cert).unwrap_or(0).to_le_bytes());
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
    let mut d: Vec<u8> = (0..8192)
        .map(|i| u8::try_from((i * 31) & 0xFF).expect("masked fixture value fits one byte"))
        .collect();
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
        self.out.ux()
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

#[cfg(windows)]
fn ping_padded_past_the_ceiling() -> Vec<u8> {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let ping = read(&root.join("System32").join("PING.EXE"));
    let mut content = vec![0u8; bundled_len()];
    content[..ping.len()].copy_from_slice(&ping);
    let cookie = wrapper_identity::PYINSTALLER_COOKIE;
    let at = content.len() - cookie.len() - COOKIE_TRAILER;
    content[at..at + cookie.len()].copy_from_slice(cookie);
    content
}

mod capture_cases;

mod contracts_cases;

mod guards_cases;

mod recovery_cases;

mod restore_cases;
