use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::clock::{self, Now};

pub const OPEN_THRESHOLD: u32 = 3;
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);
pub const DEFAULT_VERDICT_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    None,
    Opened,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Closed,
    Open,
    HalfOpenProbe,
    HalfOpenVerdict,
}

struct State {
    phase: Phase,
    open_until: SystemTime,
    verdict_since: SystemTime,
    resolve_streak: u32,
    playback_streak: u32,
}

pub struct HealthGate {
    now: Now,
    cooldown: Duration,
    verdict_timeout: Duration,
    state: Mutex<State>,
}

impl Default for HealthGate {
    fn default() -> Self {
        HealthGate::new(DEFAULT_COOLDOWN, DEFAULT_VERDICT_TIMEOUT, clock::system())
    }
}

impl HealthGate {
    pub fn new(cooldown: Duration, verdict_timeout: Duration, now: Now) -> HealthGate {
        HealthGate {
            now,
            cooldown,
            verdict_timeout,
            state: Mutex::new(State {
                phase: Phase::Closed,
                open_until: SystemTime::UNIX_EPOCH,
                verdict_since: SystemTime::UNIX_EPOCH,
                resolve_streak: 0,
                playback_streak: 0,
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn awaiting_probe_verdict(&self) -> bool {
        let now = (self.now)();
        let st = self.state();
        matches!(st.phase, Phase::HalfOpenProbe | Phase::HalfOpenVerdict)
            && elapsed(now, st.verdict_since) < self.verdict_timeout
    }

    pub fn should_short_circuit(&self, mesh_connected: bool) -> (bool, Transition) {
        let now = (self.now)();
        let mut st = self.state();
        match st.phase {
            Phase::Open if mesh_connected && now >= st.open_until => {
                st.phase = Phase::HalfOpenProbe;
                st.verdict_since = now;
                (false, Transition::None)
            }
            Phase::Open => (true, Transition::None),
            Phase::HalfOpenProbe | Phase::HalfOpenVerdict => {
                if elapsed(now, st.verdict_since) >= self.verdict_timeout {
                    close(&mut st);
                    (false, Transition::Closed)
                } else {
                    (true, Transition::None)
                }
            }
            Phase::Closed => (false, Transition::None),
        }
    }

    pub fn record_resolve_outcome(&self, healthy: bool, resolved: bool) -> Transition {
        let now = (self.now)();
        let mut st = self.state();
        match st.phase {
            Phase::Closed if healthy => {
                st.resolve_streak = 0;
                Transition::None
            }
            Phase::Closed => {
                st.resolve_streak += 1;
                if st.resolve_streak >= OPEN_THRESHOLD {
                    self.open(&mut st, now);
                    Transition::Opened
                } else {
                    Transition::None
                }
            }
            Phase::HalfOpenProbe if !healthy => {
                self.open(&mut st, now);
                Transition::None
            }
            Phase::HalfOpenProbe if resolved => {
                st.phase = Phase::HalfOpenVerdict;
                st.verdict_since = now;
                Transition::None
            }
            Phase::HalfOpenProbe => {
                close(&mut st);
                Transition::Closed
            }
            Phase::Open | Phase::HalfOpenVerdict => Transition::None,
        }
    }

    pub fn record_playback_failure(&self) -> Transition {
        let now = (self.now)();
        let mut st = self.state();
        match st.phase {
            Phase::Closed => {
                st.playback_streak += 1;
                if st.playback_streak >= OPEN_THRESHOLD {
                    self.open(&mut st, now);
                    Transition::Opened
                } else {
                    Transition::None
                }
            }
            Phase::HalfOpenProbe | Phase::HalfOpenVerdict => {
                self.open(&mut st, now);
                Transition::None
            }
            Phase::Open => Transition::None,
        }
    }

    pub fn record_playback_confirmed(&self) -> Transition {
        let mut st = self.state();
        st.playback_streak = 0;
        if st.phase == Phase::Closed {
            return Transition::None;
        }
        close(&mut st);
        Transition::Closed
    }

    fn open(&self, st: &mut State, now: SystemTime) {
        st.phase = Phase::Open;
        st.open_until = now + self.cooldown;
        st.resolve_streak = 0;
        st.playback_streak = 0;
    }
}

fn elapsed(now: SystemTime, since: SystemTime) -> Duration {
    now.duration_since(since).unwrap_or_default()
}

fn close(st: &mut State) {
    st.phase = Phase::Closed;
    st.resolve_streak = 0;
    st.playback_streak = 0;
}
