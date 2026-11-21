use super::*;

impl<M: Mesh> Ipc<M> {
    pub(super) fn is_first_party_playback(&self, url: &str) -> bool {
        self.first_party
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_playback_proxy_url(url)
    }

    pub(super) fn remember_emitted(&self, req: &ResolveRequest, emitted: &str, cid: Option<&str>) {
        let Some(map) = self.emitted.as_deref() else {
            return;
        };
        let origin = Origin {
            source_url: req.url.clone(),
            correlation_id: Some(
                cid.filter(|c| !c.is_empty())
                    .map_or_else(|| req.id.clone(), str::to_owned),
            ),
        };
        map.remember(emitted, &origin);
        map.remember_id(&req.id, &origin);
    }

    pub(super) fn remember_frame(&self, req: &ResolveRequest, frame: &[u8], cid: Option<&str>) {
        let Ok(parsed) = serde_json::from_slice::<ResolveResponse>(frame) else {
            return;
        };
        if parsed.live.is_some() || parsed.delivery.is_some() || parsed.validated.is_some() {
            self.log.line(&format!(
                "[ipc] resolved delivery id={}{} live={} delivery={} validated={}",
                req.id,
                cid_suffix(cid),
                flag(parsed.live),
                sanitize_for_console(parsed.delivery.as_deref().unwrap_or("-"), 32),
                flag(parsed.validated)
            ));
        }
        if let Some(url) = parsed.url.as_deref().filter(|u| !u.is_empty()) {
            self.remember_emitted(req, url, cid);
        }
    }
}

fn flag(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "-",
    }
}
