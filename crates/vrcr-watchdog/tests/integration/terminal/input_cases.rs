use super::*;

#[test]
fn fast_refresh_needs_motion_and_recent_traffic() {
    let mut s = AppSettings::default();
    let active = Activity {
        last_relay: Some(t0() - Duration::from_secs(1)),
        ..Activity::default()
    };
    assert!(text::fast_refresh(&active, t0(), &s, true));
    assert!(!text::fast_refresh(&active, t0(), &s, false));
    assert!(!text::fast_refresh(&idle(), t0(), &s, true));
    let stale = Activity {
        last_relay: Some(t0() - Duration::from_secs(3)),
        ..Activity::default()
    };
    assert!(!text::fast_refresh(&stale, t0(), &s, true));
    s.terminal.animations = false;
    assert!(!text::fast_refresh(&active, t0(), &s, true));
}

#[test]
fn input_buffer_edits_at_the_cursor() {
    let mut b = InputBuffer::new(Vec::new());
    for c in "helo".chars() {
        b.append(c);
    }
    b.move_left();
    b.append('l');
    assert_eq!(b.text(), "hello");
    assert_eq!(b.cursor(), 4);
    b.move_home();
    b.delete();
    assert_eq!(b.text(), "ello");
    b.move_end();
    b.backspace();
    assert_eq!(b.text(), "ell");
    assert!(b.at_end());
    assert_eq!(b.take(), "ell");
    assert_eq!(b.text(), "");
}

#[test]
fn input_buffer_caps_length_and_walks_history() {
    let mut b = InputBuffer::new(vec!["one".to_owned(), " ".to_owned(), "two".to_owned()]);
    b.previous_history();
    assert_eq!(b.text(), "two");
    b.previous_history();
    assert_eq!(b.text(), "one");
    b.previous_history();
    assert_eq!(b.text(), "one");
    b.next_history();
    assert_eq!(b.text(), "two");
    b.next_history();
    assert_eq!(b.text(), "");
    b.remember("two");
    b.previous_history();
    b.previous_history();
    assert_eq!(b.text(), "one");

    let long: String = std::iter::repeat_n('x', MAX_INPUT_CHARS + 10)
        .chain(['y'])
        .collect();
    b.set(&long);
    assert_eq!(b.text().chars().count(), MAX_INPUT_CHARS);
    assert!(b.text().ends_with('y'));
}

#[test]
fn interactive_and_animation_gates_follow_the_environment() {
    assert!(interactive_allowed(
        vrcr_watchdog::terminal::interactive::OutputSupport {
            input_is_terminal: true,
            output_supported: true
        },
        None
    ));
    assert!(!interactive_allowed(
        vrcr_watchdog::terminal::interactive::OutputSupport {
            input_is_terminal: true,
            output_supported: false
        },
        None
    ));
    assert!(!interactive_allowed(
        vrcr_watchdog::terminal::interactive::OutputSupport {
            input_is_terminal: true,
            output_supported: true
        },
        Some("TRUE")
    ));
    assert!(interactive_allowed(
        vrcr_watchdog::terminal::interactive::OutputSupport {
            input_is_terminal: true,
            output_supported: true
        },
        Some("0")
    ));
    assert!(animations_allowed(
        vrcr_watchdog::terminal::interactive::OutputSupport {
            input_is_terminal: true,
            output_supported: true
        },
        None
    ));
    assert!(!animations_allowed(
        vrcr_watchdog::terminal::interactive::OutputSupport {
            input_is_terminal: true,
            output_supported: false
        },
        None
    ));
    assert!(!animations_allowed(
        vrcr_watchdog::terminal::interactive::OutputSupport {
            input_is_terminal: true,
            output_supported: true
        },
        Some("1")
    ));
}
