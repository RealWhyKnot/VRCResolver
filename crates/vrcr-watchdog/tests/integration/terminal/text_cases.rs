use super::*;

#[test]
fn trims_keep_the_useful_end() {
    assert_eq!(trim_right("abcdef", 4), "ab..");
    assert_eq!(trim_right("abcdef", 2), "ab");
    assert_eq!(trim_right("abc", 3), "abc");
    assert_eq!(trim_right("abc", 0), "");
    assert_eq!(trim_left("abcdefgh", 6), "...fgh");
    assert_eq!(trim_left("abcdefgh", 3), "fgh");
    assert_eq!(trim_left("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 4), "...\u{e9}");
}

#[test]
fn sparkline_scales_to_the_peak_and_skips_silence() {
    assert_eq!(sparkline(&[0, 0, 0], 8, &ASCII), "");
    assert_eq!(sparkline(&[], 8, &ASCII), "");
    assert_eq!(sparkline(&[0, 50, 100], 8, &ASCII), "_=#");
    assert_eq!(sparkline(&[0, 58, 100], 8, &ASCII), "_##");
    assert_eq!(sparkline(&[1, 2, 3, 4], 2, &UNICODE).chars().count(), 2);
}

#[test]
fn wrap_breaks_on_spaces_and_hard_splits_long_words() {
    assert_eq!(blocks::wrap("aaa bbb ccc", 8), vec!["aaa bbb", "ccc"]);
    assert_eq!(blocks::wrap("abcdefghijkl", 8), vec!["abcdefgh", "ijkl"]);
    assert_eq!(
        blocks::wrap("   ", 8),
        <[std::string::String; 0]>::default()
    );
    assert_eq!(blocks::wrap("one\ntwo", 20), vec!["one", "two"]);
}

#[test]
fn state_colors_match_the_original_rules() {
    assert_eq!(blocks::state_color("pulling media"), Color::Green);
    assert_eq!(blocks::state_color("fallback"), Color::Yellow);
    assert_eq!(blocks::state_color("not eligible"), Color::Red);
    assert_eq!(blocks::state_color("off"), Color::Red);
    assert_eq!(blocks::state_color("managed"), Color::Gray);
}

#[test]
fn widths_and_ages_match_the_original_formatting() {
    assert_eq!(console_width(None), 119);
    assert_eq!(console_width(Some(10)), 20);
    assert_eq!(console_width(Some(121)), 120);
    assert_eq!(usable_width(120), 118);
    assert_eq!(usable_width(5), 20);
    assert_eq!(describe_age(None, t0()), "not yet");
    assert_eq!(
        describe_age(Some(t0() + Duration::from_secs(5)), t0()),
        "now"
    );
    assert_eq!(
        describe_age(Some(t0() - Duration::from_secs(59)), t0()),
        "59s ago"
    );
    assert_eq!(
        describe_age(Some(t0() - Duration::from_secs(61)), t0()),
        "1m ago"
    );
    assert_eq!(
        describe_age(Some(t0() - Duration::from_secs(500 * 3600)), t0()),
        "99h ago"
    );
    let mut s = AppSettings::default();
    assert_eq!(describe_terminal(&s), "status line on, motion on");
    s.terminal.animations = false;
    assert_eq!(describe_terminal(&s), "status line on, motion off");
    s.terminal.status_line = false;
    assert_eq!(describe_terminal(&s), "status line off");
}
