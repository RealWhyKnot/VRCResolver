use super::*;

impl<M: Mesh> Ipc<M> {
    pub(super) fn route_by_player_history(&self, req: &mut ResolveRequest) {
        let Some(hint) = self.hint.as_deref() else {
            return;
        };
        if self.is_first_party_playback(&req.url) {
            return;
        }
        let host = bare_host(&req.url);
        if req.player == Some(Player::AvPro)
            && self
                .stats
                .playback
                .take_master_only(&req.url, &req.id, SystemTime::now())
        {
            let next = match hint.record_non_avpro_rejection(&req.url) {
                og_hint::Rejection::TryUnity => "unity".to_owned(),
                og_hint::Rejection::Native(lease) => {
                    format!("native og_hint={}", og_hint::lease_label(lease))
                }
            };
            self.ux.log_write(
                Component::Ipc,
                &format!(
                    "player never asked past the HLS master playlist id={} host={host} next={next}",
                    req.id
                ),
            );
        }
        if req.player == Some(Player::AvPro)
            && hint.prefers_unity(&req.url)
            && profile::route_as_unity(req)
        {
            self.ux.log_write(
                Component::Ipc,
                &format!(
                    "requesting a Unity-compatible stream id={} host={host} reason=player_rejected_hls",
                    req.id
                ),
            );
        }
        if req.player == Some(Player::Unity) {
            hint.unity_served(&req.url);
        }
    }
}
