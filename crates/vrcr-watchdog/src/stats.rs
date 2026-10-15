use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use vrcr_shared::console::ResolveStatus;

pub const BANDWIDTH_BUCKETS: usize = 32;
pub const PERF_SAMPLE_CAP: usize = 2048;

pub struct Stats {
    pub started: SystemTime,
    resolves_total: AtomicU64,
    resolves_via_lh_yt: AtomicU64,
    resolves_cache_hits: AtomicU64,
    bytes_estimate_total: AtomicU64,
    reconnects: AtomicU64,
    relay_requests: AtomicU64,
    relay_bytes: AtomicU64,
    upstream_relay_requests: AtomicU64,
    upstream_relay_bytes: AtomicU64,
    last_relay_ms: AtomicU64,
    last_upstream_relay_ms: AtomicU64,
    bandwidth: Mutex<[(u64, u64); BANDWIDTH_BUCKETS]>,
    pub perf: PerfWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Activity {
    pub resolves_total: u64,
    pub resolves_via_lh_yt: u64,
    pub resolves_cache_hits: u64,
    pub bytes_estimate_total: u64,
    pub reconnects: u64,
    pub relay_requests: u64,
    pub relay_bytes: u64,
    pub upstream_relay_requests: u64,
    pub upstream_relay_bytes: u64,
    pub last_relay: Option<SystemTime>,
    pub last_upstream_relay: Option<SystemTime>,
}

impl Activity {
    pub fn relay_active(&self, now: SystemTime, window: Duration) -> bool {
        within(self.last_relay, now, window)
    }

    pub fn upstream_active(&self, now: SystemTime, window: Duration) -> bool {
        within(self.last_upstream_relay, now, window)
    }
}

fn within(at: Option<SystemTime>, now: SystemTime, window: Duration) -> bool {
    at.is_some_and(|t| now.duration_since(t).map(|d| d <= window).unwrap_or(true))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bandwidth {
    pub current: u64,
    pub peak: u64,
    pub history: Vec<u64>,
}

impl Bandwidth {
    pub fn has_traffic(&self) -> bool {
        self.peak > 0
    }
}

impl Default for Stats {
    fn default() -> Self {
        Stats::new(SystemTime::now())
    }
}

impl Stats {
    pub fn new(started: SystemTime) -> Stats {
        Stats {
            started,
            resolves_total: AtomicU64::new(0),
            resolves_via_lh_yt: AtomicU64::new(0),
            resolves_cache_hits: AtomicU64::new(0),
            bytes_estimate_total: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            relay_requests: AtomicU64::new(0),
            relay_bytes: AtomicU64::new(0),
            upstream_relay_requests: AtomicU64::new(0),
            upstream_relay_bytes: AtomicU64::new(0),
            last_relay_ms: AtomicU64::new(0),
            last_upstream_relay_ms: AtomicU64::new(0),
            bandwidth: Mutex::new([(0, 0); BANDWIDTH_BUCKETS]),
            perf: PerfWindow::default(),
        }
    }

    pub fn record_resolve(&self, via_lh_yt: bool) {
        self.resolves_total.fetch_add(1, Ordering::Relaxed);
        if via_lh_yt {
            self.resolves_via_lh_yt.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_cache_hit(&self) {
        self.resolves_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_bytes_estimate(&self, bytes: i64) {
        if let Ok(b) = u64::try_from(bytes) {
            self.bytes_estimate_total.fetch_add(b, Ordering::Relaxed);
        }
    }

    pub fn record_reconnect(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_relay_request(&self, upstream: bool) {
        self.relay_requests.fetch_add(1, Ordering::Relaxed);
        if upstream {
            self.upstream_relay_requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_relay_bytes(&self, upstream: bool, bytes: u64, now: SystemTime) {
        if bytes == 0 {
            return;
        }
        self.relay_bytes.fetch_add(bytes, Ordering::Relaxed);
        if upstream {
            self.upstream_relay_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
        let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
        let second = since.as_secs();
        {
            let mut b = self.bandwidth.lock().unwrap_or_else(|e| e.into_inner());
            let slot = &mut b[bucket(second)];
            if slot.0 != second {
                *slot = (second, 0);
            }
            slot.1 = slot.1.saturating_add(bytes);
        }
        let ms = u64::try_from(since.as_millis()).unwrap_or(u64::MAX).max(1);
        self.last_relay_ms.store(ms, Ordering::Relaxed);
        if upstream {
            self.last_upstream_relay_ms.store(ms, Ordering::Relaxed);
        }
    }

    pub fn activity(&self) -> Activity {
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        Activity {
            resolves_total: get(&self.resolves_total),
            resolves_via_lh_yt: get(&self.resolves_via_lh_yt),
            resolves_cache_hits: get(&self.resolves_cache_hits),
            bytes_estimate_total: get(&self.bytes_estimate_total),
            reconnects: get(&self.reconnects),
            relay_requests: get(&self.relay_requests),
            relay_bytes: get(&self.relay_bytes),
            upstream_relay_requests: get(&self.upstream_relay_requests),
            upstream_relay_bytes: get(&self.upstream_relay_bytes),
            last_relay: from_ms(get(&self.last_relay_ms)),
            last_upstream_relay: from_ms(get(&self.last_upstream_relay_ms)),
        }
    }

    pub fn bandwidth(&self, now: SystemTime, seconds: usize) -> Bandwidth {
        let seconds = seconds.clamp(1, BANDWIDTH_BUCKETS);
        let now_second = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let b = self.bandwidth.lock().unwrap_or_else(|e| e.into_inner());
        let history: Vec<u64> = (0..seconds)
            .map(|i| {
                let second = now_second.saturating_sub((seconds - 1 - i) as u64);
                let (s, bytes) = b[bucket(second)];
                if s == second { bytes } else { 0 }
            })
            .collect();
        Bandwidth {
            current: history.last().copied().unwrap_or(0),
            peak: history.iter().copied().max().unwrap_or(0),
            history,
        }
    }
}

fn bucket(second: u64) -> usize {
    (second % BANDWIDTH_BUCKETS as u64) as usize
}

fn from_ms(ms: u64) -> Option<SystemTime> {
    (ms > 0).then(|| UNIX_EPOCH + Duration::from_millis(ms))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayKind {
    Segment,
    Manifest,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spread {
    pub count: u64,
    pub p50: u32,
    pub p95: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Series {
    count: u64,
    max: u32,
    samples: Vec<u32>,
}

impl Series {
    pub fn push(&mut self, ms: i64) {
        let v = u32::try_from(ms.max(0)).unwrap_or(u32::MAX);
        self.count += 1;
        self.max = self.max.max(v);
        if self.samples.len() < PERF_SAMPLE_CAP {
            self.samples.push(v);
        }
    }

    pub fn spread(&mut self) -> Option<Spread> {
        if self.samples.is_empty() {
            return None;
        }
        self.samples.sort_unstable();
        let rank = |p: usize| {
            let n = self.samples.len();
            self.samples[(p * n).div_ceil(100).clamp(1, n) - 1]
        };
        Some(Spread {
            count: self.count,
            p50: rank(50),
            p95: rank(95),
            max: self.max,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerfSnapshot {
    pub relay_requests: u64,
    pub relay_errors: u64,
    pub relay_bytes: u64,
    pub segment_header: Series,
    pub segment_total: Series,
    pub manifest_total: Series,
    pub resolves: u64,
    pub resolves_cached: u64,
    pub resolves_fallback: u64,
    pub resolves_failed: u64,
    pub resolve_ms: Series,
    pub mesh_rtt: Series,
}

impl PerfSnapshot {
    pub fn is_active(&self) -> bool {
        self.relay_requests > 0 || self.resolves > 0
    }
}

#[derive(Default)]
pub struct PerfWindow {
    enabled: AtomicBool,
    window: Mutex<PerfSnapshot>,
}

impl PerfWindow {
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn record_relay(&self, kind: RelayKind, failed: bool, header_ms: i64, total_ms: i64, bytes: u64) {
        if !self.enabled() {
            return;
        }
        let mut w = self.lock();
        w.relay_requests += 1;
        w.relay_bytes = w.relay_bytes.saturating_add(bytes);
        if failed {
            w.relay_errors += 1;
            return;
        }
        match kind {
            RelayKind::Segment => {
                w.segment_header.push(header_ms);
                w.segment_total.push(total_ms);
            }
            RelayKind::Manifest => w.manifest_total.push(total_ms),
            RelayKind::Other => {}
        }
    }

    pub fn record_resolve(&self, status: ResolveStatus, via_cache: bool, elapsed: Duration) {
        if !self.enabled() {
            return;
        }
        let mut w = self.lock();
        w.resolves += 1;
        match status {
            ResolveStatus::Resolved if via_cache => w.resolves_cached += 1,
            ResolveStatus::Resolved => w
                .resolve_ms
                .push(i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)),
            ResolveStatus::Fallback => w.resolves_fallback += 1,
            ResolveStatus::Failed | ResolveStatus::Unexpected => w.resolves_failed += 1,
        }
    }

    pub fn record_mesh_rtt(&self, rtt: Duration) {
        if !self.enabled() {
            return;
        }
        self.lock()
            .mesh_rtt
            .push(i64::try_from(rtt.as_millis()).unwrap_or(i64::MAX));
    }

    pub fn take(&self) -> PerfSnapshot {
        std::mem::take(&mut *self.lock())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PerfSnapshot> {
        self.window.lock().unwrap_or_else(|e| e.into_inner())
    }
}
