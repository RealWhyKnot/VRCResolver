use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use vrcr_shared::win;

pub const RETRIES: u32 = 10;
pub const EMPTY_POLLS: u32 = 2;

pub trait Session: Send + Sync {
    fn exited(&self) -> bool;
    fn launched(&self);
    fn gave_up(&self, tries: u32);
}

pub trait Processes: Send {
    fn running(&self) -> Option<Vec<u32>>;
    fn wait_exit(&self, pid: u32, limit: Duration) -> bool;
}

pub struct VrchatProcesses;

impl Processes for VrchatProcesses {
    fn running(&self) -> Option<Vec<u32>> {
        let processes = win::snapshot_processes()?;
        Some(
            processes
                .into_iter()
                .filter(|process| win::process_stem(&process.exe).eq_ignore_ascii_case("VRChat"))
                .map(|process| process.pid)
                .collect(),
        )
    }

    fn wait_exit(&self, pid: u32, limit: Duration) -> bool {
        win::wait_for_exit(pid, limit)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub idle: Duration,
    pub exit_wait: Duration,
    pub retry: Duration,
}

impl Timing {
    pub const LIVE: Timing = Timing {
        idle: Duration::from_secs(1),
        exit_wait: Duration::from_secs(5),
        retry: Duration::from_millis(100),
    };
}

pub struct WatchHandle {
    stop: Sender<()>,
}

impl WatchHandle {
    pub fn stop(self) {
        drop(self.stop);
    }
}

pub fn spawn(
    processes: Box<dyn Processes>,
    session: Arc<dyn Session>,
    timing: Timing,
) -> Option<WatchHandle> {
    let (stop, stopped) = mpsc::channel();
    thread::Builder::new()
        .name("vrchat-exit".to_owned())
        .spawn(move || {
            let mut watcher = Watcher::new(session.as_ref(), timing);
            loop {
                let gap = watcher.poll(processes.as_ref());
                match stopped.recv_timeout(gap) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .ok()?;
    Some(WatchHandle { stop })
}

struct Watcher<'a> {
    session: &'a dyn Session,
    timing: Timing,
    seen: bool,
    paused: bool,
    empties: u32,
    failures: u32,
    claimed: Option<u32>,
}

impl<'a> Watcher<'a> {
    fn new(session: &'a dyn Session, timing: Timing) -> Watcher<'a> {
        Watcher {
            session,
            timing,
            seen: false,
            paused: false,
            empties: 0,
            failures: 0,
            claimed: None,
        }
    }

    fn poll(&mut self, processes: &dyn Processes) -> Duration {
        let Some(pids) = processes.running() else {
            self.empties = 0;
            return self.timing.retry;
        };
        if let Some(&pid) = pids.first() {
            return self.wait_on(processes, pid);
        }
        self.claimed = None;
        if self.seen && !self.paused {
            self.missing()
        } else {
            self.timing.idle
        }
    }

    fn wait_on(&mut self, processes: &dyn Processes, pid: u32) -> Duration {
        if self.paused {
            self.session.launched();
            self.paused = false;
        }
        self.seen = true;
        self.failures = 0;
        self.empties = 0;
        let started = Instant::now();
        let exited = processes.wait_exit(pid, self.timing.exit_wait);
        let stale = exited && self.claimed == Some(pid);
        self.claimed = exited.then_some(pid);
        if stale {
            self.timing.idle
        } else if exited {
            self.timing.retry
        } else {
            self.timing.retry.saturating_sub(started.elapsed())
        }
    }

    fn missing(&mut self) -> Duration {
        self.empties += 1;
        if self.empties < EMPTY_POLLS {
            return self.timing.retry;
        }
        if self.session.exited() {
            self.paused = true;
            self.empties = 0;
            return self.timing.idle;
        }
        self.failures += 1;
        if self.failures >= RETRIES {
            self.session.gave_up(self.failures);
            self.seen = false;
            self.failures = 0;
            self.empties = 0;
        }
        self.timing.retry
    }
}
