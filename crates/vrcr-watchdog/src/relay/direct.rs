use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use url::Url;
use vrcr_shared::console::{Component, Ux};

use super::activity::play_id;
use super::hitch::{TimingSample, relay_kind};
use crate::stats::RelayKind;

const RECENT_SEGMENTS: usize = 4;
const SLOW_TO_SWITCH: usize = 3;
const TRACKED_VIDEOS: usize = 64;
const PROXY_SEGMENT_PREFIX: &str = "/api/proxy/seg.";
const DIRECT_HOSTS: &[&str] = &["ttvnw.net"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Switch {
    Stay,
    ToDirect,
    ToServer,
}

#[derive(Default)]
struct Track {
    target_ms: u64,
    recent: VecDeque<bool>,
    direct: bool,
}

#[derive(Default)]
struct Tracks {
    entries: HashMap<String, Track>,
    order: VecDeque<String>,
}

impl Tracks {
    fn track(&mut self, play: &str) -> &mut Track {
        if !self.entries.contains_key(play) {
            if self.order.len() >= TRACKED_VIDEOS
                && let Some(oldest) = self.order.pop_front()
            {
                self.entries.remove(&oldest);
            }
            self.order.push_back(play.to_owned());
        }
        self.entries.entry(play.to_owned()).or_default()
    }

    fn record(&mut self, play: &str, sample: &TimingSample) -> Switch {
        match relay_kind(sample) {
            RelayKind::Manifest => {
                if let Some(media) = sample.media.filter(|m| m.live && m.target_ms > 0) {
                    self.track(play).target_ms = media.target_ms;
                }
                Switch::Stay
            }
            RelayKind::Segment => {
                let Some(track) = self.entries.get_mut(play).filter(|t| t.target_ms > 0) else {
                    return Switch::Stay;
                };
                let Some(slow) = slower_than_playback(sample, track.target_ms) else {
                    return Switch::Stay;
                };
                if track.recent.len() >= RECENT_SEGMENTS {
                    track.recent.pop_front();
                }
                track.recent.push_back(slow);
                let slow_count = track.recent.iter().filter(|s| **s).count();
                let switch = if slow_count < SLOW_TO_SWITCH {
                    Switch::Stay
                } else if track.direct {
                    Switch::ToServer
                } else if origin_of(&sample.target_url).is_some() {
                    Switch::ToDirect
                } else {
                    Switch::Stay
                };
                if switch != Switch::Stay {
                    track.direct = switch == Switch::ToDirect;
                    track.recent.clear();
                }
                switch
            }
            RelayKind::Other => Switch::Stay,
        }
    }
}

#[derive(Default)]
pub struct SlowStreams {
    tracks: Mutex<Tracks>,
}

impl SlowStreams {
    pub fn observe(&self, sample: &TimingSample, ux: &Ux) -> bool {
        if sample.suppressed || !sample.method.eq_ignore_ascii_case("GET") {
            return false;
        }
        let Some(play) = play_id(&sample.local_path) else {
            return false;
        };
        let switch = self
            .tracks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(play, sample);
        let message = match switch {
            Switch::Stay => return false,
            Switch::ToDirect => format!(
                "live video {play} is loading slower than it plays through the server; switching it to direct playback"
            ),
            Switch::ToServer => format!(
                "direct playback of live video {play} is slow too; going back through the server"
            ),
        };
        ux.log_warn(Component::Relay, &message);
        matches!(switch, Switch::ToDirect)
    }

    pub fn direct_origin(&self, play: &str, target: &str) -> Option<Url> {
        let tracks = self.tracks.lock().unwrap_or_else(|e| e.into_inner());
        if !tracks.entries.get(play).is_some_and(|t| t.direct) {
            return None;
        }
        drop(tracks);
        origin_of(target)
    }

    pub fn give_up(&self, play: &str, ux: &Ux) {
        let mut tracks = self.tracks.lock().unwrap_or_else(|e| e.into_inner());
        let Some(track) = tracks.entries.get_mut(play).filter(|t| t.direct) else {
            return;
        };
        track.direct = false;
        track.recent.clear();
        drop(tracks);
        ux.log_warn(
            Component::Relay,
            &format!("direct playback failed for live video {play}; going back through the server"),
        );
    }
}

fn slower_than_playback(sample: &TimingSample, target_ms: u64) -> Option<bool> {
    match sample.failure.as_deref() {
        Some("client_disconnect" | "shutdown") => None,
        Some(_) => Some(true),
        None if sample.status >= 400 => Some(true),
        None => Some(u64::try_from(sample.total_ms).is_ok_and(|total| total > target_ms)),
    }
}

pub fn origin_of(target: &str) -> Option<Url> {
    let proxied = Url::parse(target).ok()?;
    let path = proxied.path();
    if !path.starts_with(PROXY_SEGMENT_PREFIX) || path.ends_with(".m3u8") {
        return None;
    }
    let encoded = proxied
        .query_pairs()
        .find(|(k, _)| k == "url")
        .map(|(_, v)| v.into_owned())?;
    let decoded = URL_SAFE_NO_PAD.decode(encoded.trim_end_matches('=')).ok()?;
    let origin = Url::parse(std::str::from_utf8(&decoded).ok()?).ok()?;
    (origin.scheme() == "https" && origin.domain().is_some_and(direct_host)).then_some(origin)
}

fn direct_host(host: &str) -> bool {
    DIRECT_HOSTS.iter().any(|allowed| {
        host.strip_suffix(allowed)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}
