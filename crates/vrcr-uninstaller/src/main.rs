use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use vrcr_shared::crash::Crash;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::UtcStamp;
use vrcr_shared::paths::AppPaths;
use vrcr_shared::stop::{self, SystemOps};
use vrcr_shared::tools::{self, RealOps, RestoreOutcome};
use vrcr_shared::win::{self, Elevated};
use vrcr_shared::{hosts, wininet};
use vrcr_uninstaller::flow::{self, Host};
use vrcr_uninstaller::{SETTLE, WATCHDOG_PROCESSES, state_roots};

const VERSION: &str = match option_env!("VRCR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const CHECK_POLL: Duration = Duration::from_millis(50);

struct RealHost {
    log: Log,
    paths: AppPaths,
    ops: SystemOps,
}

impl RealHost {
    fn print(&self, line: &str, to_err: bool) {
        let result = if to_err {
            writeln!(io::stderr(), "{line}")
        } else {
            writeln!(io::stdout(), "{line}")
        };
        drop(result);
        self.log.line(line);
    }
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = CREATE_NO_WINDOW;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

impl Host for RealHost {
    fn say(&self, line: &str) {
        self.print(line, false);
    }

    fn err(&self, line: &str) {
        self.print(line, true);
    }

    fn watchdog_pids(&self) -> Vec<u32> {
        stop::pids_named(&win::list_processes(), &WATCHDOG_PROCESSES, std::process::id())
    }

    fn image_path(&self, pid: u32) -> Option<PathBuf> {
        win::process_image_path(pid)
    }

    fn alive(&self, pid: u32) -> bool {
        win::process_alive(pid)
    }

    fn stop(&self, pid: u32) -> bool {
        stop::stop(&self.ops, pid)
    }

    fn settle(&self) {
        std::thread::sleep(SETTLE);
    }

    fn tools_dir(&self) -> Option<PathBuf> {
        self.paths.vrc_tools_dir(None)
    }

    fn sweep_tools(&self, dir: &Path) {
        tools::sweep(dir, &self.log, &win::process_alive);
    }

    fn restore_tools(&self, dir: &Path) -> (RestoreOutcome, String) {
        tools::restore(dir, None, &RealOps)
    }

    fn hosts_has_entry(&self) -> bool {
        std::fs::read(hosts::hosts_path())
            .is_ok_and(|bytes| hosts::contains_bypass_entry(&String::from_utf8_lossy(&bytes)))
    }

    fn elevate(&self, exe: &Path, arg: &str, timeout: Duration) -> Elevated {
        win::run_elevated(exe, arg, timeout)
    }

    fn run_check(&self, exe: &Path, arg: &str) -> Option<i32> {
        let mut child = hidden(&mut Command::new(exe)).arg(arg).spawn().ok()?;
        let deadline = Instant::now() + CHECK_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(CHECK_POLL),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }

    fn restore_wininet(&self) -> Result<(), String> {
        let marker = self.paths.state_root().join(wininet::MARKER_FILE);
        if !marker.is_file() {
            return Ok(());
        }
        let key = win::HkcuKey::create(wininet::KEY_PATH).map_err(|e| e.to_string())?;
        let mut store = win::RegDword {
            key: &key,
            name: wininet::VALUE_NAME,
        };
        wininet::restore(Some(&mut store), &marker, &mut |line| self.say(&line)).map_err(|e| e.to_string())
    }

    fn close_log(&self) {
        self.log.close();
    }

    fn remove_dir(&self, dir: &Path) -> Result<bool, String> {
        if !dir.is_dir() {
            return Ok(false);
        }
        std::fs::remove_dir_all(dir)
            .map(|()| true)
            .map_err(|e| e.to_string())
    }

    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn utc_stamp(&self) -> UtcStamp {
        UtcStamp::at(SystemTime::now())
    }

    fn spawn_hidden_cmd(&self, args: &str, cwd: &Path) -> Result<(), String> {
        let mut cmd = Command::new("cmd.exe");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(args);
        }
        #[cfg(not(windows))]
        cmd.arg(args);
        hidden(&mut cmd)
            .current_dir(cwd)
            .spawn()
            .map(drop)
            .map_err(|e| format!("could not start cmd.exe: {e}"))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = stop::helper_main(&args) {
        std::process::exit(code);
    }
    win::ignore_ctrl_c(true);
    let paths = AppPaths::from_env();
    paths.migrate_from_legacy_product(&mut |line| println!("{line}"));
    let log = Log::install(&paths.logs_dir(), "uninstaller");
    let _crash = Crash::install(paths.crashes_dir(), "uninstaller", VERSION);
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("vrcresolver.Uninstaller.exe"));
    let install_dir = exe.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let roots = state_roots(&paths);
    let host = RealHost {
        log,
        paths,
        ops: SystemOps::new(exe),
    };
    std::process::exit(flow::run(&host, &install_dir, &roots));
}
