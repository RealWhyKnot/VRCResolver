use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime};

use vrcr_shared::crash::Crash;
use vrcr_shared::log::Log;
use vrcr_shared::paths::AppPaths;
use vrcr_shared::stop::{self, SystemOps};
use vrcr_shared::{legacy, win, wire, wrapper_identity};
use vrcr_updater::flow::{self, Host, HttpText};
use vrcr_updater::{
    ASSUME_YES_SUFFIX, DOWNLOAD_CAP, FETCH_TIMEOUT, PROMPT_TIMEOUT, PromptPolicy, USER_AGENT,
    WATCHDOG_PROCESSES, archive, prompt_policy, release, temp, wait_for_release,
};

const VERSION: &str = match option_env!("VRCR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
const KEY_Y: u16 = 0x59;

struct RealHost {
    log: Log,
    paths: AppPaths,
    helper: PathBuf,
}

fn tls() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build()
}

fn agent(global: std::time::Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(tls())
        .timeout_global(Some(global))
        .timeout_connect(Some(FETCH_TIMEOUT))
        .timeout_recv_response(Some(FETCH_TIMEOUT))
        .max_redirects(5)
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

fn drive_of(path: &Path) -> String {
    path.components().next().map_or_else(
        || "C:\\".to_owned(),
        |c| format!("{}\\", c.as_os_str().to_string_lossy()),
    )
}

fn mutex_free(name: &str) -> bool {
    match win::NamedMutex::create(name) {
        Ok(mut m) => m.try_acquire(),
        Err(_) => true,
    }
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

impl Host for RealHost {
    fn say(&self, line: &str) {
        self.print(line, false);
    }

    fn err(&self, line: &str) {
        self.print(line, true);
    }

    fn log(&self, line: &str) {
        self.log.line(line);
    }

    fn file_version(&self, path: &Path) -> Option<String> {
        wrapper_identity::version::query(path)?.file_version
    }

    fn include_prereleases(&self) -> bool {
        let text = std::fs::read_to_string(self.paths.state_root().join("settings.json")).ok();
        release::include_prereleases(text.as_deref())
    }

    fn get_text(&self, url: &str) -> Result<HttpText, String> {
        let resp = agent(FETCH_TIMEOUT)
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = resp.into_body().read_to_string().map_err(|e| e.to_string())?;
        Ok(HttpText {
            status,
            content_type,
            body,
        })
    }

    fn download(&self, url: &str, dest: &Path) -> Result<(), String> {
        self.say("Downloading\u{2026}");
        self.log(&format!("[updater] download GET {url} dest={}", dest.display()));
        let resp = agent(DOWNLOAD_CAP).get(url).call().map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("download returned HTTP {status}"));
        }
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if let Some(total) = total {
            let tmp = std::env::temp_dir();
            archive::check_free_space(total, win::free_disk_bytes(&tmp), &drive_of(&tmp))?;
            self.log(&format!("[updater] download size={} KiB", total / 1024));
        }
        let mut reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
        let mut file = File::create(dest).map_err(|e| format!("could not create {}: {e}", dest.display()))?;
        let bytes = io::copy(&mut reader, &mut file).map_err(|e| format!("download failed: {e}"))?;
        file.flush().map_err(|e| e.to_string())?;
        self.log(&format!("[updater] download complete bytes={bytes}"));
        Ok(())
    }

    fn extract(&self, zip: &Path, dest: &Path) -> Result<(), String> {
        archive::extract_zip(zip, dest)
    }

    fn prompt(&self) -> bool {
        let assume = legacy::env_with_fallback(ASSUME_YES_SUFFIX);
        match prompt_policy(assume.as_deref(), !io::stdin().is_terminal()) {
            PromptPolicy::Requested => {
                self.say("Update requested from vrcresolver; installing without a second prompt.");
                true
            }
            PromptPolicy::Redirected => {
                self.say("Update available \u{2014} stdin is redirected, declining update silently.");
                false
            }
            PromptPolicy::Ask => {
                let mut out = io::stdout();
                drop(write!(
                    out,
                    "Update available \u{2014} install now? [Y/N] (auto-N in {}s): ",
                    PROMPT_TIMEOUT.as_secs()
                ));
                drop(out.flush());
                let Some(keys) = win::KeyReader::open() else {
                    drop(writeln!(out));
                    return false;
                };
                let deadline = Instant::now() + PROMPT_TIMEOUT;
                while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                    match keys.read(left.min(std::time::Duration::from_millis(100))) {
                        Ok(Some(key)) => {
                            let echo = char::from_u32(u32::from(key.ch)).filter(|c| !c.is_control());
                            drop(writeln!(out, "{}", echo.map(String::from).unwrap_or_default()));
                            return key.vk == KEY_Y;
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
                drop(writeln!(out));
                false
            }
        }
    }

    fn stop_watchdog(&self) {
        win::ignore_ctrl_c(true);
        let own = std::process::id();
        let found = stop::pids_named(&win::list_processes(), &WATCHDOG_PROCESSES, own);
        if found.is_empty() {
            return;
        }
        let ops = SystemOps::new(self.helper.clone());
        for pid in &found {
            if !stop::stop(&ops, *pid) {
                self.log(&format!("[updater] watchdog pid={pid} did not exit"));
            }
        }
        let any_left = || !stop::pids_named(&win::list_processes(), &WATCHDOG_PROCESSES, own).is_empty();
        let free = || mutex_free(wire::WATCHDOG_MUTEX) && mutex_free(legacy::WATCHDOG_MUTEX);
        if let Err(waited) = wait_for_release(&any_left, &free, &std::thread::sleep, &Instant::now) {
            self.log(&format!(
                "[updater] watchdog-release wait timed out after {} ms -- proceeding anyway",
                waited.as_millis()
            ));
        }
    }

    fn launch_watchdog(&self, exe: &Path, dir: &Path) -> Result<(), String> {
        win::ignore_ctrl_c(false);
        let mut cmd = Command::new(exe);
        cmd.current_dir(dir)
            .env_remove(format!("VRCRESOLVER_{ASSUME_YES_SUFFIX}"))
            .env_remove(format!("WKVRCPROXY_{ASSUME_YES_SUFFIX}"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NEW_CONSOLE);
        }
        #[cfg(not(windows))]
        let _ = CREATE_NEW_CONSOLE;
        cmd.spawn()
            .map(drop)
            .map_err(|e| format!("could not relaunch {}: {e}", exe.display()))
    }

    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = stop::helper_main(&args) {
        std::process::exit(code);
    }
    let paths = AppPaths::from_env();
    paths.migrate_from_legacy_product(&mut |line| println!("{line}"));
    let log = Log::install(&paths.logs_dir(), "updater");
    let _crash = Crash::install(paths.crashes_dir(), "updater", VERSION);
    let swept = temp::sweep(&std::env::temp_dir(), SystemTime::now());
    if swept > 0 {
        log.line(&format!("[updater] swept {swept} stale temp artifacts"));
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("vrcresolver.Updater.exe"));
    let install_dir = exe.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let host = RealHost {
        log,
        paths,
        helper: exe,
    };
    std::process::exit(flow::run(&host, &install_dir));
}
