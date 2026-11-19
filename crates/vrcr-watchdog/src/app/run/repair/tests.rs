use std::fs;

use tempfile::NamedTempFile;
use vrcr_shared::console::Style;

use super::*;

fn console() -> (Ux, NamedTempFile) {
    let output = NamedTempFile::new().unwrap_or_else(|e| panic!("{e}"));
    let writer = output.reopen().unwrap_or_else(|e| panic!("{e}"));
    let ux = Ux::new(Box::new(writer), Style::plain(), String::new);
    (ux, output)
}

#[test]
fn repair_reports_when_every_step_is_ready() {
    let (ux, output) = console();
    report(&ux, Ok(false), Ok(false), Some(false), false);
    assert_eq!(
        fs::read_to_string(output.path()).unwrap_or_default(),
        "  There's nothing to repair.\n"
    );
}

#[test]
fn repair_reports_a_failed_secure_setup() {
    let (ux, output) = console();
    report(&ux, Ok(false), Err(SetupError::Failed), Some(false), false);
    assert_eq!(
        fs::read_to_string(output.path()).unwrap_or_default(),
        "  Secure video setup didn't finish. Type repair to be asked again.\n"
    );
}

#[test]
fn repair_declines_point_to_the_next_command() {
    let (ux, output) = console();
    report(
        &ux,
        Err(SetupError::Declined),
        Err(SetupError::Declined),
        Some(false),
        false,
    );
    assert_eq!(
        fs::read_to_string(output.path()).unwrap_or_default(),
        concat!(
            "  Permission for videos in public worlds was declined. Type repair to be asked again.\n",
            "  Permission for secure video was declined. Type repair to be asked again.\n",
        )
    );
}

#[test]
fn repair_summarizes_success_and_tells_the_user_if_a_restart_is_needed() {
    let (ux, output) = console();
    report(&ux, Ok(true), Ok(true), Some(true), true);
    assert_eq!(
        fs::read_to_string(output.path()).unwrap_or_default(),
        concat!(
            "  Videos from this PC can now play in public worlds. Secure video is now set up. VRCResolver is connected to VRChat's video player again.\n",
            "  Restart VRCResolver to use secure video playback.\n",
        )
    );
}
