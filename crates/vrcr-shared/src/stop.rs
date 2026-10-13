use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::win::{self, ProcessInfo};

pub const CTRL_C_ARG: &str = "--send-ctrl-c";
pub const GRACE: Duration = Duration::from_secs(5);
pub const KILL_WAIT: Duration = Duration::from_secs(2);
pub const HELPER_WAIT: Duration = Duration::from_secs(5);
const DETACHED_PROCESS: u32 = 0x0000_0008;
const POLL: Duration = Duration::from_millis(50);

pub trait ProcessOps {
    fn ctrl_c(&self, pid: u32) -> bool;
    fn wait_exit(&self, pid: u32, timeout: Duration) -> bool;
    fn terminate(&self, pid: u32) -> bool;
}

pub fn stop(ops: &dyn ProcessOps, pid: u32) -> bool {
    if ops.ctrl_c(pid) && ops.wait_exit(pid, GRACE) {
        return true;
    }
    ops.terminate(pid);
    ops.wait_exit(pid, KILL_WAIT)
}

pub fn pids_named(processes: &[ProcessInfo], stems: &[&str], own_pid: u32) -> Vec<u32> {
    processes
        .iter()
        .filter(|p| p.pid != own_pid)
        .filter(|p| {
            let stem = win::process_stem(&p.exe);
            stems.iter().any(|s| stem.eq_ignore_ascii_case(s))
        })
        .map(|p| p.pid)
        .collect()
}

pub fn helper_main(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some(CTRL_C_ARG) {
        return None;
    }
    let sent = args
        .get(1)
        .and_then(|p| p.parse::<u32>().ok())
        .is_some_and(win::send_console_ctrl_c);
    Some(if sent { 0 } else { 1 })
}

pub struct SystemOps {
    helper: PathBuf,
}

impl SystemOps {
    pub fn new(helper: PathBuf) -> SystemOps {
        SystemOps { helper }
    }
}

impl ProcessOps for SystemOps {
    fn ctrl_c(&self, pid: u32) -> bool {
        let mut cmd = Command::new(&self.helper);
        cmd.arg(CTRL_C_ARG).arg(pid.to_string());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(DETACHED_PROCESS);
        }
        #[cfg(not(windows))]
        let _ = DETACHED_PROCESS;
        let Ok(mut child) = cmd.spawn() else {
            return false;
        };
        let deadline = Instant::now() + HELPER_WAIT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    }

    fn wait_exit(&self, pid: u32, timeout: Duration) -> bool {
        win::wait_for_exit(pid, timeout)
    }

    fn terminate(&self, pid: u32) -> bool {
        win::terminate_process(pid)
    }
}
