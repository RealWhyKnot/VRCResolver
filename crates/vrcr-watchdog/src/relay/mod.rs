pub mod activity;
pub mod bypass;
pub mod headers;
pub mod hitch;
pub mod manifest;
pub mod ports;
pub mod security;
pub mod server;
pub mod targets;
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
    match url.char_indices().nth(100) {
        Some((cut, _)) => format!("{}...", &url[..cut]),
        None => url.to_owned(),
    }
}
