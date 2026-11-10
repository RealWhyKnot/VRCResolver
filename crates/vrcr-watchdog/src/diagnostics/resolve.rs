use std::time::{Instant, SystemTime};

use vrcr_shared::console::{Component, Ux};
use vrcr_shared::wire::{ResolveRequest, ResolveResponse};

use super::{PlaybackLog, host, token};

pub struct ResolveTrace<'a> {
    pub request: ResolveRequest,
    pub reason: &'static str,
    pub sent: &'static str,
    pub retried: bool,
    pub finished: bool,
    started: Instant,
    playback: &'a PlaybackLog,
    ux: &'a Ux,
}

impl<'a> ResolveTrace<'a> {
    pub fn new(request: &ResolveRequest, playback: &'a PlaybackLog, ux: &'a Ux) -> Self {
        playback.resolving(request, SystemTime::now(), ux);
        Self {
            request: request.clone(),
            reason: "request_cancelled",
            sent: "none",
            retried: false,
            finished: false,
            started: Instant::now(),
            playback,
            ux,
        }
    }

    pub fn reply(&mut self, frame: &[u8], cached: bool) {
        match serde_json::from_slice::<ResolveResponse>(frame) {
            Ok(response) => {
                self.playback.resolved(
                    &self.request,
                    &response,
                    super::ResolveDelivery {
                        cached,
                        retried: self.retried,
                        bytes: frame.len(),
                    },
                    SystemTime::now(),
                    self.ux,
                );
                self.finished = true;
                if response.action != "resolved" {
                    self.playback.close(
                        &self.request.id,
                        "resolve_fallback",
                        SystemTime::now(),
                        self.ux,
                    );
                }
            }
            Err(_) => {
                self.reason = "reply_parse_failed";
                self.sent = "received_frame";
            }
        }
    }
}

impl Drop for ResolveTrace<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.ux.write(Component::Ipc, &format!(
            "resolve failed req={} kind=resolve host={} status=unknown elapsed_ms={} bytes=0 retry={} sent={} reason={}",
            token(&self.request.id), host(&self.request.url), self.started.elapsed().as_millis(), self.retried, self.sent, self.reason,
        ));
        self.playback.close(
            &self.request.id,
            "resolve_error",
            SystemTime::now(),
            self.ux,
        );
    }
}
