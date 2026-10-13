#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use vrcr_watchdog::clock::Now;

#[derive(Clone)]
pub struct ManualClock(Arc<Mutex<SystemTime>>);

impl ManualClock {
    pub fn at(t: SystemTime) -> ManualClock {
        ManualClock(Arc::new(Mutex::new(t)))
    }

    pub fn utc(secs_since_epoch: u64) -> ManualClock {
        ManualClock::at(UNIX_EPOCH + Duration::from_secs(secs_since_epoch))
    }

    pub fn now(&self) -> Now {
        let c = self.0.clone();
        Arc::new(move || *c.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn get(&self) -> SystemTime {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn advance(&self, d: Duration) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *g += d;
    }
}

pub const T_2026_05_22: u64 = 1_779_444_000;
pub const T_2026_09_29: u64 = 1_790_704_800;
pub const T_2026_08_21: u64 = 1_787_313_600;
