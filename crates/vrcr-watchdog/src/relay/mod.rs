pub mod activity;
pub mod bypass;
pub mod headers;
pub mod hitch;
pub mod live;
pub mod manifest;
pub mod ports;
pub mod retry;
pub mod security;
pub mod server;
pub mod targets;
pub mod tls;
pub mod upstream;

pub use activity::RelayActivity;
pub use bypass::RelayBypass;
pub use hitch::{HitchDetector, HitchDiagnostic, TimingSample};
pub use ports::RelayPortManager;
pub use server::{RelayConfig, RelayServer};
pub use targets::{RelayTarget, TargetKind, TargetResolver};
pub use upstream::{Upstream, UpstreamError, UpstreamRequest, UpstreamResponse};

pub fn short_url(url: &str) -> String {
    if url.is_empty() {
        return "?".to_owned();
    }
    vrcr_shared::logutil::redact_url(url)
}
