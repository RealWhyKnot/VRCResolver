use crate::diagnostics::resolve::ResolveTrace;

use super::*;

impl<M: Mesh> Ipc<M> {
    pub(super) fn is_native_fallback_host(&self, url: &str) -> bool {
        self.native_fallback
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .matches_url(url)
    }

    pub(super) async fn answer_native_fallback<S: AsyncWrite + Unpin>(
        &self,
        pipe: &mut S,
        trace: &mut ResolveTrace<'_>,
        host: &str,
        cid: Option<&str>,
    ) {
        let id = trace.request.id.clone();
        let frame = framing::fallback_frame(&id, fallback::NATIVE_FALLBACK_HOST);
        trace.reply(&frame, false);
        if framing::write_frame(pipe, &frame).await.is_err() {
            trace.finished = false;
            trace.reason = "wrapper_disconnected";
            trace.sent = "partial_or_none";
        }
        self.ux.skipped(host, fallback::NATIVE_FALLBACK_HOST, None);
        self.ux.log_write(
            Component::Ipc,
            &format!(
                "native fallback (site blocks the server) id={id}{} host={host}",
                cid_suffix(cid)
            ),
        );
    }
}

pub(super) fn is_native_fallback_reply(result: &MeshResult) -> bool {
    result.action == action::FALLBACK_NATIVE
        && result.reason.as_deref() == Some(fallback::NATIVE_FALLBACK_HOST)
}
