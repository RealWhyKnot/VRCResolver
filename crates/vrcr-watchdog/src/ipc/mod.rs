pub mod framing;
#[cfg(windows)]
pub mod pipe;
pub mod server;

use std::future::Future;
use tokio::time::Instant;

use vrcr_shared::wire::ResolveRequest;

pub use server::{Ipc, IpcBuilder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshResult {
    pub frame: Vec<u8>,
    pub action: String,
    pub reason: Option<String>,
    pub retry_after_ms: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    pub url: String,
    pub kind: &'static str,
    pub ms_since_open: i32,
    pub delivered_height: Option<i32>,
    pub detail: Option<String>,
    pub correlation_id: Option<String>,
}

pub trait Mesh: Send + Sync + 'static {
    fn current_node_host(&self) -> String;
    fn is_connected(&self) -> bool;
    fn resolve(
        &self,
        req: &ResolveRequest,
        deadline: Instant,
    ) -> impl Future<Output = Result<MeshResult, String>> + Send;
    fn send_playback_feedback(&self, feedback: Feedback);
}

pub fn caller_opted_into_v2(req: &ResolveRequest) -> bool {
    req.protocol_version.is_some()
        || req.correlation_id.as_deref().is_some_and(|c| !c.is_empty())
        || req.accept_protocols.is_some()
        || req.accept_codecs.is_some()
        || req.max_audio_channels.is_some()
        || req.prefer_highest.is_some()
        || req.vrchat_format_arg.as_deref().is_some_and(|f| !f.is_empty())
}
