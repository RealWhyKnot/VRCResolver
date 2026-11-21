use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Instant;

use vrcr_watchdog::diagnostics::startup;
use vrcr_watchdog::maintenance::patch::{PatchSession, VRCHAT_EXIT};
use vrcr_watchdog::monitoring::vrchat::{self, Processes, Session, Timing};

use super::*;

const WAIT: Duration = Duration::from_secs(5);
const FAST: Timing = Timing {
    idle: Duration::from_millis(10),
    exit_wait: Duration::from_millis(500),
    retry: Duration::from_millis(5),
};

#[derive(Default)]
struct Table {
    pids: Vec<u32>,
    waits: u32,
}

#[derive(Clone, Default)]
struct FakeVrchat {
    table: Arc<(Mutex<Table>, Condvar)>,
}

impl FakeVrchat {
    fn running(pids: &[u32]) -> FakeVrchat {
        let fake = FakeVrchat::default();
        fake.set(pids);
        fake
    }

    fn set(&self, pids: &[u32]) {
        let (table, changed) = &*self.table;
        let mut table = table.lock().expect("fake process table");
        table.pids = pids.to_vec();
        table.waits = 0;
        drop(table);
        changed.notify_all();
    }

    fn exit_after_the_watcher_waits(&self) {
        let (table, changed) = &*self.table;
        let (mut table, waited) = changed
            .wait_timeout_while(table.lock().expect("fake process table"), WAIT, |table| {
                table.waits == 0
            })
            .expect("fake process table");
        assert!(!waited.timed_out(), "the watcher never waited on VRChat");
        table.pids.clear();
        drop(table);
        changed.notify_all();
    }
}

impl Processes for FakeVrchat {
    fn running(&self) -> Option<Vec<u32>> {
        Some(
            self.table
                .0
                .lock()
                .expect("fake process table")
                .pids
                .clone(),
        )
    }

    fn wait_exit(&self, pid: u32, limit: Duration) -> bool {
        !self
            .table
            .1
            .wait_timeout_while(self.enter_wait(), limit, |table| table.pids.contains(&pid))
            .expect("fake process table")
            .0
            .pids
            .contains(&pid)
    }
}

impl FakeVrchat {
    fn enter_wait(&self) -> MutexGuard<'_, Table> {
        let (table, changed) = &*self.table;
        let mut entered = table.lock().expect("fake process table");
        entered.waits += 1;
        changed.notify_all();
        entered
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Exited(bool),
    Launched,
    GaveUp(u32),
}

struct Recording<S> {
    inner: S,
    events: Sender<Event>,
}

impl<S: Session> Session for Recording<S> {
    fn exited(&self) -> bool {
        let paused = self.inner.exited();
        self.events
            .send(Event::Exited(paused))
            .expect("test is listening");
        paused
    }

    fn launched(&self) {
        self.inner.launched();
        self.events
            .send(Event::Launched)
            .expect("test is listening");
    }

    fn gave_up(&self, tries: u32) {
        self.inner.gave_up(tries);
        self.events
            .send(Event::GaveUp(tries))
            .expect("test is listening");
    }
}

fn recording<S: Session + 'static>(inner: S) -> (Arc<dyn Session>, Receiver<Event>) {
    let (events, received) = mpsc::channel();
    (Arc::new(Recording { inner, events }), received)
}

fn patched(r: &Rig) -> Vec<u8> {
    r.write_wrapper();
    let og = vrc_bundled();
    write(&r.backup, &og);
    write(&r.target, &wrapper_bytes());
    og
}

#[test]
fn vrchat_exit_restores_tools_so_the_next_start_sees_a_clean_stop() {
    let r = Rig::new();
    let og = patched(&r);
    let m = r.manager();
    assert_eq!(startup::begin(&r.state), "unknown");
    let game = FakeVrchat::running(&[42]);
    let (session, events) = recording(PatchSession::new(Arc::clone(&m), &r.state, r.ux()));
    let watch = vrchat::spawn(Box::new(game.clone()), session, FAST).expect("watcher thread");
    game.exit_after_the_watcher_waits();
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Exited(true)));
    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), og);
    assert!(r.flag().is_file());
    watch.stop();
    drop(m);

    let next = r.manager();
    assert_eq!(startup::begin(&r.state), VRCHAT_EXIT);
    next.recover_from_unclean_shutdown();
    assert!(!r.flag().exists());
    assert_eq!(read(&r.target), og);
    let output = r.out.logs();
    assert!(!output.contains("Recovery:"), "{output}");
    assert!(!output.contains("[warn]"), "{output}");
}

#[test]
fn vrchat_relaunch_in_the_same_session_resumes_patching() {
    let r = Rig::new();
    let og = patched(&r);
    let m = r.manager();
    assert_eq!(startup::begin(&r.state), "unknown");
    let game = FakeVrchat::running(&[42]);
    let (session, events) = recording(PatchSession::new(Arc::clone(&m), &r.state, r.ux()));
    let watch = vrchat::spawn(Box::new(game.clone()), session, FAST).expect("watcher thread");
    game.exit_after_the_watcher_waits();
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Exited(true)));
    assert!(m.paused_for_vrchat());
    assert!(!m.wrapper_missing());
    assert_eq!(m.repair_wrapper(), Some(false));
    m.tick_once();
    m.tick_once();
    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), og);

    game.set(&[43]);
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Launched));
    assert!(!m.paused_for_vrchat());
    assert!(!r.flag().exists());
    assert_eq!(
        fs::read_to_string(r.state.join(startup::MARKER)).unwrap_or_default(),
        "running"
    );
    m.tick_once();
    assert_eq!(read(&r.target), wrapper_bytes());
    assert_eq!(read(&r.backup), og);

    game.set(&[]);
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Exited(true)));
    assert_eq!(r.entries(), vec!["yt-dlp.exe".to_owned()]);
    assert_eq!(read(&r.target), og);
    watch.stop();
    assert_eq!(startup::begin(&r.state), VRCHAT_EXIT);
}

struct Flaky {
    failures_left: AtomicU32,
}

impl Session for Flaky {
    fn exited(&self) -> bool {
        self.failures_left
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_err()
    }

    fn launched(&self) {}

    fn gave_up(&self, _tries: u32) {}
}

#[test]
fn a_failed_restore_after_vrchat_exits_is_retried_until_it_works() {
    let game = FakeVrchat::running(&[7]);
    let (session, events) = recording(Flaky {
        failures_left: AtomicU32::new(2),
    });
    let watch = vrchat::spawn(Box::new(game.clone()), session, FAST).expect("watcher thread");
    game.exit_after_the_watcher_waits();
    let seen: Vec<Event> = (0..3)
        .map(|_| events.recv_timeout(WAIT).expect("exit attempt"))
        .collect();
    assert_eq!(
        seen,
        vec![
            Event::Exited(false),
            Event::Exited(false),
            Event::Exited(true)
        ]
    );
    game.set(&[8]);
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Launched));
    watch.stop();
}

#[test]
fn the_watcher_never_restores_before_it_has_seen_vrchat_run() {
    let r = Rig::new();
    let og = patched(&r);
    let m = r.manager();
    let game = FakeVrchat::running(&[]);
    let (session, events) = recording(PatchSession::new(Arc::clone(&m), &r.state, r.ux()));
    let watch = vrchat::spawn(Box::new(game), session, FAST).expect("watcher thread");
    assert!(events.recv_timeout(Duration::from_millis(100)).is_err());
    assert_eq!(read(&r.target), wrapper_bytes());
    assert_eq!(read(&r.backup), og);
    assert!(!m.paused_for_vrchat());
    assert!(!r.flag().exists());
    watch.stop();
}

struct Scripted {
    script: Mutex<VecDeque<Option<Vec<u32>>>>,
    exits_at_once: bool,
    polls: Sender<Instant>,
}

fn scripted(steps: &[Option<&[u32]>], exits_at_once: bool) -> (Box<Scripted>, Receiver<Instant>) {
    let (polls, received) = mpsc::channel();
    let script = steps.iter().map(|step| step.map(<[u32]>::to_vec)).collect();
    let fake = Scripted {
        script: Mutex::new(script),
        exits_at_once,
        polls,
    };
    (Box::new(fake), received)
}

impl Processes for Scripted {
    fn running(&self) -> Option<Vec<u32>> {
        let mut script = self.script.lock().expect("fake process script");
        let step = if script.len() > 1 {
            script.pop_front().flatten()
        } else {
            script.front().cloned().flatten()
        };
        drop(script);
        let _ = self.polls.send(Instant::now());
        step
    }

    fn wait_exit(&self, _pid: u32, _limit: Duration) -> bool {
        self.exits_at_once
    }
}

#[test]
fn a_single_failed_or_empty_snapshot_mid_session_keeps_the_patch() {
    let r = Rig::new();
    let og = patched(&r);
    let m = r.manager();
    let (game, polls) = scripted(
        &[
            Some(&[42]),
            None,
            Some(&[42]),
            Some(&[]),
            Some(&[42]),
            Some(&[]),
            None,
            Some(&[]),
            Some(&[42]),
        ],
        false,
    );
    let (session, events) = recording(PatchSession::new(Arc::clone(&m), &r.state, r.ux()));
    let watch = vrchat::spawn(game, session, FAST).expect("watcher thread");
    for _ in 0..12 {
        polls.recv_timeout(WAIT).expect("watcher polls");
    }
    assert!(events.try_recv().is_err());
    assert!(!m.paused_for_vrchat());
    assert_eq!(read(&r.target), wrapper_bytes());
    assert_eq!(read(&r.backup), og);
    watch.stop();
}

#[test]
fn a_wait_that_returns_at_once_while_vrchat_runs_never_spins() {
    let (game, polls) = scripted(&[Some(&[42])], true);
    let (session, events) = recording(Flaky {
        failures_left: AtomicU32::new(0),
    });
    let watch = vrchat::spawn(game, session, FAST).expect("watcher thread");
    let stamps: Vec<Instant> = (0..6)
        .map(|_| polls.recv_timeout(WAIT).expect("watcher polls"))
        .collect();
    watch.stop();
    let gaps: Vec<Duration> = stamps
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .collect();
    assert!(gaps[0] >= FAST.retry, "{gaps:?}");
    assert!(gaps[1..].iter().all(|gap| *gap >= FAST.idle), "{gaps:?}");
    assert!(events.try_recv().is_err());
}

#[test]
fn the_watcher_gives_up_after_every_restore_retry_fails() {
    let game = FakeVrchat::running(&[7]);
    let (session, events) = recording(Flaky {
        failures_left: AtomicU32::new(u32::MAX),
    });
    let watch = vrchat::spawn(Box::new(game.clone()), session, FAST).expect("watcher thread");
    game.exit_after_the_watcher_waits();
    let seen: Vec<Event> = (0..=vrchat::RETRIES)
        .map(|_| events.recv_timeout(WAIT).expect("exit attempt"))
        .collect();
    let mut expected: Vec<Event> = (0..vrchat::RETRIES).map(|_| Event::Exited(false)).collect();
    expected.push(Event::GaveUp(vrchat::RETRIES));
    assert_eq!(seen, expected);
    assert!(events.recv_timeout(FAST.idle * 5).is_err());
    game.set(&[8]);
    game.exit_after_the_watcher_waits();
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Exited(false)));
    watch.stop();
}

#[test]
fn giving_up_on_a_restore_says_why_in_the_log() {
    let r = Rig::new();
    let session = PatchSession::new(r.manager(), &r.state, r.ux());
    session.gave_up(vrchat::RETRIES);
    let output = r.out.logs();
    assert!(
        output.contains("its own yt-dlp.exe could not be put back after 10 tries"),
        "{output}"
    );
    assert!(output.contains("[warn]"), "{output}");
}
