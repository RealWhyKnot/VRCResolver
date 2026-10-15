use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vrcr_shared::log::Log;
use vrcr_shared::win::{self, MemoryStatus, ProcessUsage};

use crate::clock::Now;
use crate::monitoring::heartbeat::format_uptime;
use crate::stats::{Activity, PerfSnapshot, Series, Stats};

pub const INTERVAL: Duration = Duration::from_secs(60);

const MIB: f64 = 1024.0 * 1024.0;
const GIB: f64 = MIB * 1024.0;

pub fn mib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / MIB)
}

fn gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / GIB)
}

pub fn cpu_percent(before_100ns: u64, after_100ns: u64, wall: Duration, cores: usize) -> Option<f64> {
    let capacity = (wall.as_nanos() / 100).saturating_mul(cores.max(1) as u128);
    (capacity > 0).then(|| after_100ns.saturating_sub(before_100ns) as f64 * 100.0 / capacity as f64)
}

fn push_spread(out: &mut String, key: &str, series: &mut Series) {
    if let Some(s) = series.spread() {
        out.push_str(&format!(
            " {key}=p50:{},p95:{},max:{},n:{}",
            s.p50, s.p95, s.max, s.count
        ));
    }
}

fn push_usage(out: &mut String, usage: Option<&ProcessUsage>) {
    if let Some(u) = usage {
        out.push_str(&format!(
            " ws_mib={} private_mib={} handles={}",
            mib(u.working_set),
            mib(u.private_bytes),
            u.handles
        ));
    }
}

pub fn perf_line(
    window: Duration,
    snap: &mut PerfSnapshot,
    usage: Option<&ProcessUsage>,
    cpu: Option<f64>,
) -> String {
    let mut s = format!("[perf] window={}s", window.as_secs());
    if snap.relay_requests > 0 {
        s.push_str(&format!(
            " relay_req={} relay_err={} relay_mib={}",
            snap.relay_requests,
            snap.relay_errors,
            mib(snap.relay_bytes)
        ));
    }
    push_spread(&mut s, "seg_header_ms", &mut snap.segment_header);
    push_spread(&mut s, "seg_total_ms", &mut snap.segment_total);
    push_spread(&mut s, "manifest_ms", &mut snap.manifest_total);
    if snap.resolves > 0 {
        s.push_str(&format!(" resolves={}", snap.resolves));
        for (key, n) in [
            ("cached", snap.resolves_cached),
            ("fallback", snap.resolves_fallback),
            ("failed", snap.resolves_failed),
        ] {
            if n > 0 {
                s.push_str(&format!(" {key}={n}"));
            }
        }
    }
    push_spread(&mut s, "resolve_ms", &mut snap.resolve_ms);
    push_spread(&mut s, "mesh_rtt_ms", &mut snap.mesh_rtt);
    if let Some(c) = cpu {
        s.push_str(&format!(" cpu={c:.1}%"));
    }
    push_usage(&mut s, usage);
    s
}

pub fn session_line(up: Duration, a: &Activity, usage: Option<&ProcessUsage>) -> String {
    let mut s = format!(
        "[session] up={} resolves={} cached={} local_video={} relay_req={} relay_mib={} upstream_mib={} reconnects={}",
        format_uptime(up),
        a.resolves_total,
        a.resolves_cache_hits,
        a.resolves_via_lh_yt,
        a.relay_requests,
        mib(a.relay_bytes),
        mib(a.upstream_relay_bytes),
        a.reconnects
    );
    if let Some(u) = usage {
        s.push_str(&format!(
            " cpu_s={:.1} peak_ws_mib={}",
            u.cpu_100ns as f64 / 10_000_000.0,
            mib(u.peak_working_set)
        ));
    }
    push_usage(&mut s, usage);
    s
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub cpu: Option<String>,
    pub cores: usize,
    pub memory: Option<MemoryStatus>,
    pub disk_free: Option<u64>,
    pub settings: String,
}

impl SystemInfo {
    pub fn probe(state_root: &std::path::Path, settings: String) -> SystemInfo {
        SystemInfo {
            cpu: win::cpu_name(),
            cores: logical_cores(),
            memory: win::memory_status(),
            disk_free: win::free_disk_bytes(state_root),
            settings,
        }
    }
}

pub fn logical_cores() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

pub fn system_line(info: &SystemInfo) -> String {
    let mut s = "[startup] system".to_owned();
    if let Some(cpu) = &info.cpu {
        s.push_str(&format!(" cpu=\"{cpu}\""));
    }
    s.push_str(&format!(" cores={}", info.cores));
    if let Some(m) = info.memory {
        s.push_str(&format!(
            " ram_gib={} ram_free_gib={}",
            gib(m.total),
            gib(m.available)
        ));
    }
    if let Some(free) = info.disk_free {
        s.push_str(&format!(" disk_free_gib={}", gib(free)));
    }
    s.push_str(&format!(" settings={}", info.settings));
    s
}

pub struct Phases {
    started: Instant,
    last: Instant,
    marks: Vec<(&'static str, Duration)>,
}

impl Default for Phases {
    fn default() -> Self {
        Phases::start()
    }
}

impl Phases {
    pub fn start() -> Phases {
        let now = Instant::now();
        Phases {
            started: now,
            last: now,
            marks: Vec::new(),
        }
    }

    pub fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.marks.push((name, now - self.last));
        self.last = now;
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.marks.iter().map(|(n, _)| *n).collect()
    }

    pub fn line(&self) -> String {
        let mut s = "[startup] phases".to_owned();
        for (name, took) in &self.marks {
            s.push_str(&format!(" {name}={}ms", took.as_millis()));
        }
        s.push_str(&format!(" total={}ms", (self.last - self.started).as_millis()));
        s
    }
}

pub struct PerfReporter {
    pub stats: Arc<Stats>,
    pub cores: usize,
    pub now: Now,
    pub log: Log,
}

#[derive(Debug, Clone, Copy)]
struct Mark {
    at: Instant,
    cpu_100ns: Option<u64>,
}

impl PerfReporter {
    pub fn spawn(self: Arc<Self>, interval: Duration, cancel: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut mark = Mark {
                at: Instant::now(),
                cpu_100ns: win::process_usage().map(|u| u.cpu_100ns),
            };
            loop {
                let stopping = tokio::select! {
                    () = cancel.cancelled() => true,
                    () = tokio::time::sleep(interval) => false,
                };
                mark = self.tick(mark);
                if stopping {
                    self.log.line(&self.session());
                    return;
                }
            }
        })
    }

    fn tick(&self, previous: Mark) -> Mark {
        let at = Instant::now();
        let usage = win::process_usage();
        let mut snap = self.stats.perf.take();
        if snap.is_active() {
            let cpu = match (previous.cpu_100ns, usage) {
                (Some(before), Some(u)) => cpu_percent(before, u.cpu_100ns, at - previous.at, self.cores),
                _ => None,
            };
            self.log
                .line(&perf_line(at - previous.at, &mut snap, usage.as_ref(), cpu));
        }
        Mark {
            at,
            cpu_100ns: usage.map(|u| u.cpu_100ns),
        }
    }

    pub fn session(&self) -> String {
        let up = (self.now)()
            .duration_since(self.stats.started)
            .unwrap_or_default();
        session_line(up, &self.stats.activity(), win::process_usage().as_ref())
    }
}
