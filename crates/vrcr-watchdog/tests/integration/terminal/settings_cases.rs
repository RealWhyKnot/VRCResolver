use super::*;

#[test]
fn activity_glyph_and_pulse_follow_motion_settings() {
    assert_eq!(
        activity_glyph(
            &ASCII,
            vrcr_watchdog::terminal::text::AnimationState {
                active: false,
                animated: true
            },
            3
        ),
        ""
    );
    assert_eq!(
        activity_glyph(
            &ASCII,
            vrcr_watchdog::terminal::text::AnimationState {
                active: true,
                animated: false
            },
            3
        ),
        "*"
    );
    assert_eq!(
        activity_glyph(
            &ASCII,
            vrcr_watchdog::terminal::text::AnimationState {
                active: true,
                animated: true
            },
            5
        ),
        "/"
    );
    assert_eq!(pulse_color(0, 0, false), Color::DarkGray);
    assert_eq!(pulse_color(1, 2, true), Color::Green);
    assert_eq!(pulse_color(4, 0, true), Color::Gray);
}
