use std::path::{Path, PathBuf};
use std::time::Duration;

use vrcr_shared::tools::RestoreOutcome;
use vrcr_shared::win::Elevated;
use vrcr_shared::{hosts, relay_tls};

use crate::{
    CloseReport, HOSTS_TIMEOUT, TLS_TIMEOUT, WATCHDOG_EXE, close_watchdogs, rmdir_log_name, self_delete_args,
};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERRORS: i32 = 2;

pub trait Host {
    fn say(&self, line: &str);
    fn err(&self, line: &str);
    fn watchdog_pids(&self) -> Vec<u32>;
    fn image_path(&self, pid: u32) -> Option<PathBuf>;
    fn alive(&self, pid: u32) -> bool;
    fn stop(&self, pid: u32) -> bool;
    fn settle(&self);
    fn tools_dir(&self) -> Option<PathBuf>;
    fn sweep_tools(&self, dir: &Path);
    fn restore_tools(&self, dir: &Path) -> (RestoreOutcome, String);
    fn hosts_has_entry(&self) -> bool;
    fn elevate(&self, exe: &Path, arg: &str, timeout: Duration) -> Elevated;
    fn run_check(&self, exe: &Path, arg: &str) -> Option<i32>;
    fn restore_wininet(&self) -> Result<(), String>;
    fn close_log(&self);
    fn remove_dir(&self, dir: &Path) -> Result<bool, String>;
    fn temp_dir(&self) -> PathBuf;
    fn utc_stamp(&self) -> vrcr_shared::logutil::UtcStamp;
    fn spawn_hidden_cmd(&self, args: &str, cwd: &Path) -> Result<(), String>;
}

fn run_step(host: &dyn Host, step: &str, body: impl FnOnce() -> Result<(), String>) -> u32 {
    host.say(&format!("[uninstall] {step} start"));
    match body() {
        Ok(()) => {
            host.say(&format!("[uninstall] {step} ok"));
            0
        }
        Err(e) => {
            host.err(&format!("[uninstall] {step} ERROR {e}"));
            1
        }
    }
}

pub fn close_watchdog(host: &dyn Host, install_dir: &Path) -> Result<CloseReport, String> {
    let pids = host.watchdog_pids();
    let report = close_watchdogs(
        &pids,
        install_dir,
        &|pid| host.image_path(pid),
        &|pid| host.alive(pid),
        &|pid| host.stop(pid),
    );
    host.say(&report.summary());
    if let Some(e) = report.error() {
        return Err(e);
    }
    if report.matched > 0 {
        host.settle();
    }
    Ok(report)
}

pub fn restore_ytdlp(host: &dyn Host, tools: Option<&Path>) -> Result<(), String> {
    let Some(tools) = tools else {
        host.say("[uninstall] restore-yt-dlp skipped: VRChat Tools dir not found");
        return Ok(());
    };
    host.sweep_tools(tools);
    let (outcome, detail) = host.restore_tools(tools);
    host.sweep_tools(tools);
    match outcome {
        RestoreOutcome::Failed => {
            host.err(&format!(
                "[uninstall] WARNING: yt-dlp.exe was not restored ({detail}) -- check Tools/yt-dlp.exe by hand; deleting it makes VRChat re-download its own on next launch."
            ));
            Err(format!("Tools/yt-dlp.exe could not be restored -- {detail}"))
        }
        RestoreOutcome::RemovedOrphanWrapper => {
            host.say(
                "[uninstall] yt-dlp-og.exe was missing; removed our Tools/yt-dlp.exe so VRChat redownloads its yt-dlp on next session.",
            );
            Ok(())
        }
        other => {
            host.say(&format!("[uninstall] restore-yt-dlp {other:?}: {detail}"));
            Ok(())
        }
    }
}

fn elevated_exit(
    step: &str,
    result: Elevated,
    timeout: Duration,
    declined: &str,
    host: &dyn Host,
) -> Result<(), String> {
    match result {
        Elevated::Exited(0) => Ok(()),
        Elevated::Exited(code) => Err(format!("elevated {step} exited with code {code}")),
        Elevated::Declined => {
            host.say(declined);
            Ok(())
        }
        Elevated::TimedOut => {
            host.say(&format!(
                "[uninstall] {step} elevation child still running after {}s",
                timeout.as_secs()
            ));
            Ok(())
        }
        Elevated::Failed(e) => Err(format!("could not start the elevated {step}: {e}")),
    }
}

pub fn remove_hosts(host: &dyn Host, watchdog: &Path) -> Result<(), String> {
    if !watchdog.is_file() {
        host.say("[uninstall] remove-hosts skipped: watchdog exe missing (can't re-exec elevated)");
        return Ok(());
    }
    if !host.hosts_has_entry() {
        host.say("[uninstall] remove-hosts skipped: entry already absent");
        return Ok(());
    }
    let result = host.elevate(watchdog, hosts::REMOVE_ARG, HOSTS_TIMEOUT);
    elevated_exit(
        "remove-hosts",
        result,
        HOSTS_TIMEOUT,
        "UAC declined \u{2014} hosts entry left in place. Remove it manually if desired.",
        host,
    )
}

pub fn remove_relay_tls(host: &dyn Host, watchdog: &Path) -> Result<(), String> {
    if !watchdog.is_file() {
        host.say("[uninstall] remove-relay-tls skipped: watchdog exe missing (can't re-exec elevated)");
        return Ok(());
    }
    if host.run_check(watchdog, relay_tls::CHECK_ARG) == Some(relay_tls::CHECK_CLEAN) {
        host.say("[uninstall] remove-relay-tls skipped: no relay certificate or legacy binding left");
        return Ok(());
    }
    let result = host.elevate(watchdog, relay_tls::REMOVE_ARG, TLS_TIMEOUT);
    elevated_exit(
        "remove-relay-tls",
        result,
        TLS_TIMEOUT,
        "UAC declined -- localhost.youtube.com HTTPS certificate/bindings may be left in place.",
        host,
    )
}

pub fn wipe_state(host: &dyn Host, roots: &[PathBuf]) -> Result<(), String> {
    host.close_log();
    let mut wiped = 0;
    for root in roots {
        match host.remove_dir(root) {
            Ok(true) => wiped += 1,
            Ok(false) => {}
            Err(e) => host.err(&format!("[uninstall] could not wipe {}: {e}", root.display())),
        }
    }
    host.say(&format!("[uninstall] wipe-state directories_wiped={wiped}"));
    Ok(())
}

pub fn schedule_self_delete(host: &dyn Host, install_dir: &Path) -> Result<(), String> {
    let temp = host.temp_dir();
    let log = temp.join(rmdir_log_name(&host.utc_stamp()));
    let args = self_delete_args(install_dir, &log)?;
    host.spawn_hidden_cmd(&args, &temp)?;
    host.say(&format!(
        "[uninstall] schedule-self-delete cmd-log={}",
        log.display()
    ));
    Ok(())
}

pub fn run(host: &dyn Host, install_dir: &Path, roots: &[PathBuf]) -> i32 {
    let watchdog = install_dir.join(WATCHDOG_EXE);
    host.say(&format!("[uninstall] start installDir={}", install_dir.display()));

    if run_step(host, "close-watchdog", || {
        close_watchdog(host, install_dir).map(drop)
    }) > 0
    {
        host.say("Uninstall stopped at close-watchdog -- see the message above.");
        return EXIT_ERRORS;
    }
    let tools = host.tools_dir();
    let mut errors = 0;
    errors += run_step(host, "restore-yt-dlp", || restore_ytdlp(host, tools.as_deref()));
    errors += run_step(host, "remove-hosts", || remove_hosts(host, &watchdog));
    errors += run_step(host, "remove-relay-tls", || remove_relay_tls(host, &watchdog));
    errors += run_step(host, "restore-wininet-limit", || host.restore_wininet());
    errors += run_step(host, "wipe-state", || wipe_state(host, roots));
    errors += run_step(host, "schedule-self-delete", || {
        schedule_self_delete(host, install_dir)
    });

    if errors == 0 {
        host.say("VRCResolver uninstalled. The install folder will disappear in a moment.");
        EXIT_OK
    } else {
        host.say(&format!(
            "Uninstall finished with {errors} non-fatal error(s) \u{2014} see messages above."
        ));
        EXIT_ERRORS
    }
}
