use super::non_avpro::{GENERIC_PLAYER_ERROR, player_error_url};
use super::*;

impl VrcLogMonitor {
    pub(super) fn process_line(&mut self, line: &str, now: SystemTime) {
        if line.contains("[Behaviour] OnLeftRoom") || line.contains("[Behaviour] Entering Room:") {
            self.stop_watching("world_change", now);
            return;
        }
        if let Some(c) = AUDIO_DEVICE_COUNT.captures(line) {
            self.on_audio_count(&c[1] == "0");
            return;
        }
        if let Some(c) = AVPRO_OPENING.captures(line) {
            self.on_opening(c[1].to_owned(), now);
            return;
        }
        if let Some((_, resolved)) = line.split_once("[Video Playback] URL '")
            && let Some((source, result)) = resolved.split_once("' resolved to '")
            && let Some((result, _)) = result.split_once('\'')
        {
            self.on_native_resolution(source, result, now);
            return;
        }
        if let Some((_, height)) = parse_observed_resolution(line)
            && let Some((url, at)) = self.active.clone()
        {
            self.remember_delivered_height(&url, height, now);
            self.confirm_playback(&url);
            self.confirm_route(&url);
            self.start_playing(&url, at, now);
            self.send(&url, feedback::PLAYING, elapsed_ms(at, now), Some(height));
            return;
        }
        let lower = line.to_ascii_lowercase();
        self.observe_untrusted_urls(line, &lower);
        if line.contains("[AVProVideo] Error:") {
            self.player_error = Some(crate::diagnostics::player_error(line));
            if line.contains("Loading failed") {
                self.on_load_failed(now, true);
            }
            return;
        }
        if let Some(kind) = world_player_error(line, &lower) {
            if let Some((url, parsed)) = player_error_url(line) {
                if parsed.host_str().is_some_and(source_url::is_spotify_host)
                    && self.on_unrequested_spotify_error(url, &lower, now)
                {
                    return;
                }
                if self.on_non_avpro_error(url, kind, now) {
                    return;
                }
            } else if kind == GENERIC_PLAYER_ERROR {
                return;
            }
            if self.player_error.is_none() {
                self.player_error = Some(kind.to_owned());
            }
            self.on_load_failed(now, false);
            return;
        }
        if line.contains("[AVProVideo] Using playback path:") {
            if let Some((url, _)) = self.active.clone() {
                self.confirm_playback(&url);
            }
            self.stall = None;
            return;
        }
        if let Some(announced) = announced_url(line) {
            let keep = announced.is_some_and(|url| {
                self.stall
                    .as_ref()
                    .is_some_and(|stall| !self.stall_matches(stall, url))
            });
            if !keep {
                self.stall = None;
            }
        }
    }

    fn on_native_resolution(&mut self, source: &str, result: &str, now: SystemTime) {
        if self.wrapper_hint_shown || !external_video_url(result) {
            return;
        }
        if self
            .deps
            .stats
            .as_ref()
            .is_some_and(|stats| !stats.playback.recently_requested(source, now))
            && self
                .deps
                .wrapper_missing
                .as_ref()
                .is_some_and(|check| check())
        {
            self.deps.ux.event(Notice::VideoWrapperMissing);
            self.wrapper_hint_shown = true;
        }
    }

    pub(super) fn on_audio_count(&mut self, missing: bool) {
        let ux = &self.deps.ux;
        if missing && !self.audio_missing {
            let audio = (self.deps.audio)();
            if audio.blocks_playback() {
                ux.event(vrcr_shared::console::Notice::NoSound);
                ux.log_error(
                    Component::VrcLog,
                    &format!(
                        "VRChat has no sound output: {}. Videos will fail to load until you {NO_SOUND_FIX}",
                        audio::no_sound_reason(&audio)
                    ),
                );
            } else {
                ux.event(vrcr_shared::console::Notice::AudioUnavailable);
                ux.log_warn(
                    Component::VrcLog,
                    "VRChat found no audio devices; AVPro videos can fail to load until Windows audio recovers",
                );
            }
        } else if !missing && self.audio_missing {
            ux.event(vrcr_shared::console::Notice::AudioRestored);
            ux.log_write(Component::VrcLog, "VRChat's audio devices are back");
        }
        self.audio_missing = missing;
    }

    pub(super) fn source_of(&self, url: &str) -> Option<String> {
        if local::is_play_url(url) {
            return self.deps.local.as_ref().and_then(|plays| plays.source(url));
        }
        self.origin(&self.canonical(url))
            .or_else(|| self.origin(url))
            .map(|origin| origin.source_url)
            .or_else(|| self.relay_source(url))
    }

    pub(super) fn on_opening(&mut self, opened: String, now: SystemTime) {
        self.remember_avpro_source(&opened, now);
        let canonical = self.canonical(&opened);
        if let Some(stats) = &self.deps.stats {
            stats.playback.reset_video_failure(&canonical);
        }
        if let Some((previous, _)) = &self.active
            && let Some(stats) = &self.deps.stats
        {
            stats.playback.close(
                self.active_playback_id.as_deref().unwrap_or(previous),
                "replaced",
                now,
                &self.deps.ux,
            );
        }
        self.active_playback_id = self
            .deps
            .stats
            .as_ref()
            .and_then(|stats| stats.playback.context(&canonical))
            .map(|context| context.playback_id);
        self.player_error = None;
        self.last_opening = Some((opened.clone(), now));
        self.active = Some((canonical.clone(), now));
        self.playing = None;
        self.stall = Some(Stall {
            url: opened,
            at: now,
        });
        if self.delivered_height(&canonical, now).is_some() {
            self.start_playing(&canonical, now, now);
        }
    }

    pub(super) fn on_load_failed(&mut self, now: SystemTime, from_avpro: bool) {
        let recent = self
            .last_opening
            .take_if(|(_, at)| {
                now.duration_since(*at)
                    .map_or(true, |d| d <= AVPRO_OPEN_FAIL_WINDOW)
            })
            .or_else(|| self.stall.as_ref().map(|s| (s.url.clone(), s.at)))
            .or_else(|| self.active.clone().filter(|_| from_avpro));
        if let Some((observed, at)) = recent {
            self.report_load_failure(&observed, elapsed_ms(at, now), now);
        } else {
            self.stall = None;
            self.deps.log.line(&format!(
                "[vrclog] {} with no open video error={}",
                if from_avpro {
                    "load_failure"
                } else {
                    "world player error"
                },
                self.player_error.as_deref().unwrap_or("loading_failed")
            ));
            return;
        }
        if let Some((url, _)) = &self.active
            && let Some(stats) = &self.deps.stats
        {
            stats.playback.close(
                self.active_playback_id.as_deref().unwrap_or(url),
                "error",
                now,
                &self.deps.ux,
            );
        }
        self.clear_playback();
    }

    pub(super) fn report_load_failure(&mut self, observed: &str, ms: i32, now: SystemTime) {
        let failed = self.canonical(observed);
        let first_failure = self
            .deps
            .stats
            .as_ref()
            .is_none_or(|stats| stats.playback.first_video_failure(&failed, now));
        if first_failure {
            self.deps.ux.video(
                vrcr_shared::console::VideoStatus::Failed,
                &failed,
                "VRChat couldn't load the video.",
            );
        }
        if external_video_url(observed) && self.setup_hint_due(now) {
            self.deps.ux.event(Notice::PublicVideoSetupMissing);
            self.setup_hint.shown = true;
        }
        if self.audio_missing {
            let audio = (self.deps.audio)();
            if audio.blocks_playback() {
                self.deps.ux.log_error(
                    Component::VrcLog,
                    &format!(
                        "video failed to load because VRChat has no sound output: {}. To fix it, {NO_SOUND_FIX} (not a resolver fault)",
                        audio::no_sound_reason(&audio)
                    ),
                );
                self.deps.log.line(&format!(
                    "[vrclog] load_failure_no_sound_output ms={ms} state={} url={}",
                    audio.state.name(),
                    redact_url(&failed)
                ));
                return;
            }
        }
        let height = self.delivered_height(&failed, now);
        if self.first_failure_report(&failed, now) {
            self.send(&failed, feedback::LOAD_FAILURE, ms, height);
        }
        let recovery = self.mark_playback_failure(&failed);
        self.deps.ux.log_write(
            Component::VrcLog,
            &format!(
                "load_failure ms={ms} url={}{} error={}",
                redact_url(&failed),
                recovery.suffix(),
                self.player_error.as_deref().unwrap_or("loading_failed"),
            ),
        );
    }

    pub(super) fn first_failure_report(&mut self, url: &str, now: SystemTime) -> bool {
        self.reported_failures.retain(|_, at| {
            now.duration_since(*at)
                .is_ok_and(|age| age < REPEAT_FAILURE_QUIET)
        });
        if self.reported_failures.contains_key(url) {
            return false;
        }
        if self.reported_failures.len() >= MAX_REPORTED_FAILURES
            && let Some(oldest) = self
                .reported_failures
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(known, _)| known.clone())
        {
            self.reported_failures.remove(&oldest);
        }
        self.reported_failures.insert(url.to_owned(), now);
        true
    }

    pub(super) fn clear_playback(&mut self) {
        self.active_playback_id = None;
        self.stall = None;
        self.playing = None;
        self.active = None;
    }

    pub fn next_deadline(&self) -> Option<SystemTime> {
        let stall = self
            .stall
            .as_ref()
            .map(|s| s.at + self.deps.silent_stall_window);
        let playing = self.playing.as_ref().map(|p| p.next);
        match (stall, playing) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn fire_due(&mut self, now: SystemTime) {
        if let Some(stats) = &self.deps.stats {
            stats.playback.expire(now, &self.deps.ux);
        }
        if self
            .stall
            .as_ref()
            .is_some_and(|s| now >= s.at + self.deps.silent_stall_window)
            && let Some(stall) = self.stall.take()
        {
            self.on_silent_stall(&stall.url, elapsed_ms(stall.at, now), now);
        }
        if self.playing.as_ref().is_some_and(|p| now >= p.next)
            && let Some(p) = self.playing.take()
            && let Some(height) = self.delivered_height(&p.url, now)
        {
            self.send(
                &p.url,
                feedback::PLAYING,
                elapsed_ms(p.opened_at, now),
                Some(height),
            );
            self.playing = Some(Playing {
                next: now + PLAYING_FEEDBACK_INTERVAL,
                ..p
            });
        }
    }

    pub(super) fn on_silent_stall(&mut self, opened: &str, ms: i32, now: SystemTime) {
        let reported = self.canonical(opened);
        self.deps.ux.video(
            vrcr_shared::console::VideoStatus::Stalled,
            &reported,
            "The video stopped loading in VRChat.",
        );
        if self.handle_wedged_relay(opened, ms) {
            self.playing = None;
            self.active = None;
            return;
        }
        let height = self.delivered_height(&reported, now);
        self.send(&reported, feedback::SILENT_STALL, ms, height);
        if let Some(stats) = &self.deps.stats {
            stats.playback.close(
                self.active_playback_id.as_deref().unwrap_or(&reported),
                "error",
                now,
                &self.deps.ux,
            );
        }
        self.playing = None;
        self.active = None;
        let recovery = self.mark_playback_failure(&reported);
        self.deps.ux.log_warn(
            Component::VrcLog,
            &format!(
                "silent_stall ms={ms} url={}{}",
                redact_url(&reported),
                recovery.suffix()
            ),
        );
    }

    pub fn handle_wedged_relay(&self, opened: &str, ms: i32) -> bool {
        let Some(wedge) = &self.deps.relay_wedge else {
            return false;
        };
        let Some(port) = wedge.activity.wedged_port(opened) else {
            return false;
        };
        let handled = (wedge.on_wedged)(port);
        self.deps.ux.log_warn(
            Component::VrcLog,
            &format!(
                "relay_pool_wedged ms={ms} port={port}: VRChat never reached the local relay{}",
                if handled {
                    ""
                } else {
                    "; counting as a failed play"
                }
            ),
        );
        handled
    }

    pub fn mark_playback_failure(&self, reported: &str) -> Recovery {
        let is_local = local::is_play_url(reported);
        let attributed = !is_local && self.is_attributed(reported);
        let mut og_hint_armed = false;
        let source = if is_local {
            self.deps
                .local
                .as_ref()
                .and_then(|plays| plays.failed(reported))
        } else {
            self.origin(reported).map(|o| o.source_url)
        };
        let mut evicted = 0;
        if let Some(source) = source {
            let (count, lease) = crate::og_hint::arm(
                self.deps.cache.as_deref(),
                self.deps.og_hint.as_deref(),
                &source,
            );
            evicted += count;
            og_hint_armed = lease.is_some();
        }
        evicted += self
            .deps
            .cache
            .as_ref()
            .map_or(0, |c| c.evict_by_url(reported));
        if attributed
            && let Some(gate) = &self.deps.health
            && gate.record_playback_failure() == Transition::Opened
        {
            self.deps.ux.resolver_paused();
            self.deps.ux.log_warn(
                Component::VrcLog,
                &format!(
                    "resolver paused -- {} playbacks failed in a row; VRChat's own resolver takes over while things recover",
                    health::OPEN_THRESHOLD
                ),
            );
        }
        Recovery {
            evicted,
            og_hint_armed,
        }
    }

    pub(super) fn send(&self, url: &str, kind: &'static str, ms: i32, height: Option<i32>) {
        if local::is_play_url(url) {
            return;
        }
        let origin = self.origin(url);
        self.deps.mesh.send_playback_feedback(Feedback {
            url: url.to_owned(),
            kind,
            ms_since_open: ms,
            delivered_height: height,
            detail: match kind {
                feedback::LOAD_FAILURE => Some(
                    self.player_error
                        .clone()
                        .unwrap_or_else(|| "loading_failed".to_owned()),
                ),
                feedback::SILENT_STALL => Some("silent_stall".to_owned()),
                _ => None,
            },
            correlation_id: origin.as_ref().and_then(|o| o.correlation_id.clone()),
            source_url: origin.map(|o| o.source_url),
        });
    }

    pub(super) fn start_playing(&mut self, url: &str, opened_at: SystemTime, now: SystemTime) {
        if url.trim().is_empty() || self.playing.as_ref().is_some_and(|p| p.url == url) {
            return;
        }
        self.playing = Some(Playing {
            url: url.to_owned(),
            opened_at,
            next: now + PLAYING_FEEDBACK_INTERVAL,
        });
    }

    pub(super) fn remember_delivered_height(&mut self, url: &str, height: i32, now: SystemTime) {
        if url.trim().is_empty() || height <= 0 {
            return;
        }
        self.delivered.insert(url.to_owned(), (height, now));
        self.delivered
            .retain(|_, (_, at)| !delivered_height_expired(*at, now));
        while self.delivered.len() > MAX_DELIVERED_HEIGHT_ENTRIES {
            let Some(oldest) = self
                .delivered
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.delivered.remove(&oldest);
        }
    }

    pub(super) fn delivered_height(&mut self, url: &str, now: SystemTime) -> Option<i32> {
        if url.trim().is_empty() {
            return None;
        }
        let (height, at) = *self.delivered.get(url)?;
        if delivered_height_expired(at, now) {
            self.delivered.remove(url);
            return None;
        }
        Some(height)
    }

    pub(super) fn stop_watching(&mut self, reason: &str, now: SystemTime) {
        let watched = self
            .active
            .take()
            .map(|(u, _)| u)
            .or_else(|| self.last_opening.as_ref().map(|(u, _)| u.clone()));
        self.stall = None;
        self.playing = None;
        self.last_opening = None;
        if let Some(url) = watched {
            if let Some(stats) = &self.deps.stats {
                stats.playback.close(
                    self.active_playback_id.take().as_deref().unwrap_or(&url),
                    "player_closed",
                    now,
                    &self.deps.ux,
                );
            }
            self.deps.log.line(&format!(
                "[vrclog] stopped watching {} reason={reason}",
                redact_url(&url)
            ));
        }
    }
}

const ANNOUNCE_MARKERS: [&str; 2] = ["Load Url:", "Now Playing:"];

fn delivered_height_expired(at: SystemTime, now: SystemTime) -> bool {
    now.duration_since(at)
        .is_ok_and(|age| age > DELIVERED_HEIGHT_TTL)
}

fn announced_url(line: &str) -> Option<Option<&str>> {
    let rest = ANNOUNCE_MARKERS
        .iter()
        .find_map(|marker| line.split_once(marker).map(|(_, rest)| rest))?;
    Some(
        rest.split_whitespace()
            .next()
            .filter(|token| token.contains("://")),
    )
}

fn world_player_error(line: &str, lower: &str) -> Option<&'static str> {
    if lower.contains("ratelimited") || lower.contains("rate limited") {
        return None;
    }
    if line.contains("Video Error: PlayerError") {
        Some("protv_player_error")
    } else if lower.contains("nowplaying") && line.contains("\"type\":\"mediaError\"") {
        Some("protv_media_error")
    } else if line.contains("Video error event: PlayerError")
        || line.contains("Error code: Player Error")
    {
        Some("videotxl_player_error")
    } else if lower.contains("usharpvideo")
        && (lower.contains("video error") || lower.contains("video failed"))
    {
        Some("usharpvideo_error")
    } else if line
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word == "PlayerError")
    {
        Some(GENERIC_PLAYER_ERROR)
    } else {
        None
    }
}

fn external_video_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| {
        url.host_str().is_some_and(|host| {
            !host
                .trim_end_matches('.')
                .eq_ignore_ascii_case(GATEWAY_HOST)
        })
    })
}
