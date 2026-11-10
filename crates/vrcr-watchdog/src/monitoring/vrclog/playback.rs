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
        if let Some((_, height)) = parse_observed_resolution(line)
            && let Some((url, at)) = self.active.clone()
        {
            self.remember_delivered_height(&url, height, now);
            self.confirm_playback(&url);
            self.start_playing(&url, at, now);
            self.send(&url, feedback::PLAYING, elapsed_ms(at, now), Some(height));
            return;
        }
        if line.contains("[AVProVideo] Error:") {
            self.player_error = Some(crate::diagnostics::player_error(line));
            if line.contains("Loading failed") {
                self.on_load_failed(now);
            }
            return;
        }
        let accepted = line.contains("[AVProVideo] Using playback path:");
        if accepted || line.contains("Now Playing:") || line.contains("Load Url:") {
            if accepted && let Some((url, _)) = self.active.clone() {
                self.confirm_playback(&url);
            }
            self.stall = None;
        }
    }

    pub(super) fn on_audio_count(&mut self, missing: bool) {
        let ux = &self.deps.ux;
        if missing && !self.audio_missing {
            let audio = (self.deps.audio)();
            if audio.blocks_playback() {
                ux.error(
                    Component::VrcLog,
                    &format!(
                        "VRChat has no sound output: {}. Videos will fail to load until you {NO_SOUND_FIX}",
                        audio::no_sound_reason(&audio)
                    ),
                );
            } else {
                ux.warn(
                    Component::VrcLog,
                    "VRChat found no audio devices; AVPro videos can fail to load until Windows audio recovers",
                );
            }
        } else if !missing && self.audio_missing {
            ux.write(Component::VrcLog, "VRChat's audio devices are back");
        }
        self.audio_missing = missing;
    }

    pub(super) fn on_opening(&mut self, opened: String, now: SystemTime) {
        let canonical = self.canonical(&opened);
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

    pub(super) fn on_load_failed(&mut self, now: SystemTime) {
        let recent = self.last_opening.take_if(|(_, at)| {
            now.duration_since(*at)
                .map_or(true, |d| d <= AVPRO_OPEN_FAIL_WINDOW)
        });
        if let Some((observed, at)) = recent {
            self.report_load_failure(&observed, elapsed_ms(at, now), now);
        } else {
            self.stall = None;
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
        if self.audio_missing {
            let audio = (self.deps.audio)();
            if audio.blocks_playback() {
                self.deps.ux.error(
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
        self.send(&failed, feedback::LOAD_FAILURE, ms, height);
        let recovery = self.mark_playback_failure(&failed);
        self.deps.ux.write(
            Component::VrcLog,
            &format!(
                "load_failure ms={ms} url={}{} error={}",
                redact_url(&failed),
                recovery.suffix(),
                self.player_error.as_deref().unwrap_or("loading_failed"),
            ),
        );
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
        if self.handle_wedged_relay(opened, ms) {
            self.playing = None;
            self.active = None;
            return;
        }
        let reported = self.canonical(opened);
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
        self.deps.ux.warn(
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
        self.deps.ux.warn(
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
            self.deps
                .cache
                .as_ref()
                .and_then(|cache| cache.source_url_for_resolved(reported))
        };
        if let (Some(hint), Some(source)) = (&self.deps.og_hint, source) {
            hint.record_load_failure(&source);
            og_hint_armed = true;
        }
        let evicted = self
            .deps
            .cache
            .as_ref()
            .map_or(0, |c| c.evict_by_url(reported));
        if attributed
            && let Some(gate) = &self.deps.health
            && gate.record_playback_failure() == Transition::Opened
        {
            self.deps.ux.warn(
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
            correlation_id: None,
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
        self.delivered.retain(|_, (_, at)| !expired(*at, now));
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
        if expired(at, now) {
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
