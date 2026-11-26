use super::*;

const AVPRO_SOURCE_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_AVPRO_SOURCES: usize = 64;
pub(super) const GENERIC_PLAYER_ERROR: &str = "world_player_error";

impl VrcLogMonitor {
    pub(super) fn opened_matches(&self, opened: &str, line_url: &str) -> bool {
        let canonical = self.canonical(opened);
        source_url::same_source(line_url, opened)
            || source_url::same_source(line_url, &canonical)
            || self
                .origin(&canonical)
                .is_some_and(|o| source_url::same_source(line_url, &o.source_url))
            || self
                .relay_source(opened)
                .is_some_and(|source| source_url::same_source(line_url, &source))
    }

    fn matches_open_video(&self, line_url: &str) -> bool {
        [
            self.last_opening.as_ref().map(|(url, _)| url.as_str()),
            self.stall.as_ref().map(|stall| stall.url.as_str()),
            self.active.as_ref().map(|(url, _)| url.as_str()),
        ]
        .into_iter()
        .flatten()
        .any(|opened| self.opened_matches(opened, line_url))
    }

    pub(super) fn remember_avpro_source(&mut self, opened: &str, now: SystemTime) {
        let source = self.source_of(opened).unwrap_or_else(|| opened.to_owned());
        self.avpro_sources.retain(|_, at| {
            now.duration_since(*at)
                .is_ok_and(|age| age <= AVPRO_SOURCE_TTL)
        });
        if self.avpro_sources.len() >= MAX_AVPRO_SOURCES
            && let Some(oldest) = self
                .avpro_sources
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(known, _)| known.clone())
        {
            self.avpro_sources.remove(&oldest);
        }
        self.avpro_sources
            .insert(source_url::normalize(&source), now);
    }

    fn opened_by_avpro(&self, source: &str, now: SystemTime) -> bool {
        self.avpro_sources
            .get(&source_url::normalize(source))
            .is_some_and(|at| {
                now.duration_since(*at)
                    .is_ok_and(|age| age <= AVPRO_SOURCE_TTL)
            })
    }

    pub(super) fn on_non_avpro_error(
        &mut self,
        url: &str,
        kind: &'static str,
        now: SystemTime,
    ) -> bool {
        let source = self.source_of(url).unwrap_or_else(|| url.to_owned());
        let ours = self
            .deps
            .stats
            .as_ref()
            .is_some_and(|stats| stats.playback.recently_requested(&source, now));
        if !ours || self.opened_by_avpro(&source, now) || self.matches_open_video(url) {
            if kind == GENERIC_PLAYER_ERROR
                && !self.matches_open_video(url)
                && !self.matches_open_video(&source)
            {
                self.deps.log.line(&format!(
                    "[vrclog] world player error for another video url={} error={kind}",
                    redact_url(&source)
                ));
                return true;
            }
            return false;
        }
        let first_failure = self
            .deps
            .stats
            .as_ref()
            .is_none_or(|stats| stats.playback.first_video_failure(&source, now));
        if first_failure {
            self.deps.ux.video(
                vrcr_shared::console::VideoStatus::Failed,
                &source,
                "VRChat couldn't load the video.",
            );
        }
        if self.first_failure_report(&source, now) {
            let origin = self.origin(url);
            self.deps.mesh.send_playback_feedback(Feedback {
                url: url.to_owned(),
                kind: feedback::LOAD_FAILURE,
                ms_since_open: 0,
                delivered_height: None,
                detail: Some(kind.to_owned()),
                correlation_id: origin.and_then(|o| o.correlation_id),
                source_url: Some(source.clone()),
            });
        }
        let evicted = self
            .deps
            .cache
            .as_ref()
            .map_or(0, |c| c.evict_by_url(&source));
        let next = match self
            .deps
            .og_hint
            .as_ref()
            .map(|hint| hint.record_non_avpro_rejection(&source))
        {
            Some(Rejection::TryUnity) => "unity".to_owned(),
            Some(Rejection::Native(lease)) => {
                format!("native og_hint={}", crate::og_hint::lease_label(lease))
            }
            None => "none".to_owned(),
        };
        self.deps.ux.log_write(
            Component::VrcLog,
            &format!(
                "load_failure player=non_avpro url={} evicted={evicted} next={next} error={kind}",
                redact_url(&source)
            ),
        );
        true
    }
}
