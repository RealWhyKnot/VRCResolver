use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use vrcr_shared::console::{Banner, Color, Outcome, ResolveStatus, Style, Ux, VideoStatus};

static ROW: AtomicUsize = AtomicUsize::new(0);

fn clock() -> String {
    [
        "22:27:08", "22:27:11", "22:27:33", "22:31:03", "22:31:05", "02:07:13", "02:09:08",
        "02:09:34", "02:09:40", "02:09:41", "02:15:12",
    ][ROW.load(Ordering::Relaxed)]
    .to_owned()
}

fn main() {
    let ux = Ux::new(Box::new(std::io::stdout()), Style::plain(), clock).with_width(|| Some(120));
    ux.banner(&Banner {
        version: "2026.11.13",
        channel: None,
        sha: "",
        build_time: "",
        dev: false,
        paths: &[],
    });
    ux.notice(
        Color::Gray,
        "VRChat is open. Setup finishes once no video is loading.",
    );
    ROW.store(1, Ordering::Relaxed);
    ux.notice(
        Color::White,
        "Ready. Videos in VRChat now play through VRCResolver.",
    );
    ROW.store(2, Ordering::Relaxed);
    ux.resolve_outcome(&Outcome {
        host: "youtube.com",
        player: "AVPro",
        height: Some(1080),
        status: ResolveStatus::Resolved,
        via_cache: false,
        via_lh_yt: false,
        elapsed: Duration::from_millis(3200),
        reason: None,
        public_message: None,
    });
    ROW.store(3, Ordering::Relaxed);
    ux.skipped(
        "youtube.com",
        "all_configs_failed",
        Some("The video is private."),
    );
    ROW.store(4, Ordering::Relaxed);
    ux.video(
        VideoStatus::Failed,
        "youtube.com",
        "VRChat couldn't play it either.",
    );
    ROW.store(5, Ordering::Relaxed);
    ux.notice(
        Color::Yellow,
        "Lost the connection to the server. Trying again...",
    );
    ROW.store(6, Ordering::Relaxed);
    ux.skipped("example.trycloudflare.com", "server_unreachable", None);
    ROW.store(7, Ordering::Relaxed);
    ux.video(
        VideoStatus::Stalled,
        "example.trycloudflare.com",
        "The video stopped loading in VRChat.",
    );
    ROW.store(8, Ordering::Relaxed);
    ux.notice(Color::Gray, "Connected again after 2m 27s.");
    ROW.store(9, Ordering::Relaxed);
    ux.skipped(
        "example.trycloudflare.com",
        "unity_unsupported_format",
        None,
    );
    ROW.store(10, Ordering::Relaxed);
    ux.playback_error(
        "youtube.com",
        &vrcr_shared::console::PlaybackError::proxy("rate_limited"),
    );
}
