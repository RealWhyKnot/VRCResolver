use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::log::Log;

use crate::clock::Now;
use crate::display::format_bytes;
use crate::stats::{Activity, Stats};

pub const INTERVAL: Duration = Duration::from_secs(30 * 60);
pub const QUIET_WINDOW: Duration = Duration::from_secs(5 * 60);

pub fn format_uptime(up: Duration) -> String {
    let secs = up.as_secs();
    let (days, hours, minutes) = (secs / 86_400, secs / 3_600, secs / 60);
    if days >= 1 {
        format!("{days}d{}h", hours % 24)
    } else if hours >= 1 {
        format!("{hours}h{}m", minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

pub fn line(up: Duration, mesh_connected: bool, a: &Activity, cache_size: usize) -> String {
    let mut s = format!(
        "up={} mesh={}",
        format_uptime(up),
        if mesh_connected {
            "connected"
        } else {
            "disconnected"
        }
    );
    if a.resolves_total > 0 {
        s.push_str(&format!(" resolves={}", a.resolves_total));
        if a.resolves_via_lh_yt > 0 {
            s.push_str(&format!(" ({} local video)", a.resolves_via_lh_yt));
        }
    }
    if a.bytes_estimate_total > 0 {
        s.push_str(&format!(" video={}", format_bytes(a.bytes_estimate_total)));
    }
    if cache_size > 0 || a.resolves_cache_hits > 0 {
        s.push_str(&format!(" cache={cache_size}({}hits)", a.resolves_cache_hits));
    }
    if a.reconnects > 0 {
        s.push_str(&format!(" reconnects={}", a.reconnects));
    }
    s
}

pub struct Heartbeat {
    pub stats: Arc<Stats>,
    pub mesh_connected: Box<dyn Fn() -> bool + Send + Sync>,
    pub cache_size: Box<dyn Fn() -> usize + Send + Sync>,
    pub now: Now,
    pub ux: Ux,
    pub log: Log,
}

impl Heartbeat {
    pub fn tick(&self) -> Option<String> {
        let now = (self.now)();
        let recent = self
            .log
            .last_write()
            .is_some_and(|t| now.duration_since(t).map_or(true, |d| d < QUIET_WINDOW));
        if recent {
            return None;
        }
        let up = now.duration_since(self.stats.started).unwrap_or_default();
        let text = line(
            up,
            (self.mesh_connected)(),
            &self.stats.activity(),
            (self.cache_size)(),
        );
        self.ux.write(Component::Heartbeat, &text);
        Some(text)
    }

    pub fn spawn(self: Arc<Self>, interval: Duration, cancel: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(interval) => {}
                }
                self.tick();
            }
        })
    }
}
