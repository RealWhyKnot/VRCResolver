use super::*;

#[test]
fn unknown_playback_reason_is_a_bounded_ascii_token() {
    assert_eq!(
        console::PlaybackError::proxy("invalid_range!")
            .message()
            .as_deref(),
        Some("playback error: invalid_range")
    );
    let hostile = format!("\x1b[31m{}\x1b[0m\r\n\u{2603}/?!", "a_0".repeat(100));
    let error = console::PlaybackError::proxy(&hostile);
    assert_eq!(
        error.message().as_deref(),
        Some(format!("playback error: {}", "a_0".repeat(32)).as_str())
    );
    assert_eq!(
        console::PlaybackError::proxy("\x1b[31m\u{2603}")
            .message()
            .as_deref(),
        Some("playback error: unknown")
    );
}

#[test]
fn resolve_public_messages_are_last_and_empty_messages_add_nothing() {
    let mut sample = outcome(ResolveStatus::Fallback);
    sample.reason = Some("all_configs_failed");
    let (_, original) = console::resolve_outcome_line("*", &sample);
    for message in [None, Some(""), Some(" \t\r\n")] {
        sample.public_message = message;
        assert_eq!(console::resolve_outcome_line("*", &sample).1, original);
    }
    sample.public_message = Some("The video is private.");
    assert_eq!(
        console::resolve_outcome_line("*", &sample).1,
        format!("{original} -- The video is private.")
    );
}

#[test]
fn resolve_public_messages_strip_terminal_controls_and_private_urls() {
    let message = "\x1b\x1b[31mThe\x1b[0m\t video\r\n is\x00 private. \u{2603}\x1b]0;title\x07\u{9b}31m\u{9d}title\u{9c}\x1b(B https://example.com/secret?token=secret";
    let sample = Outcome {
        public_message: Some(message),
        ..outcome(ResolveStatus::Fallback)
    };
    let (_, line) = console::resolve_outcome_line("*", &sample);
    assert!(line.ends_with(" -- The video is private. example.com"));
    assert!(line.is_ascii());
    assert!(!line.chars().any(char::is_control));
    assert!(!line.contains("secret"));
}

#[test]
fn resolve_public_messages_are_capped_at_two_hundred_characters() {
    let message = "x".repeat(2000);
    let sample = Outcome {
        public_message: Some(&message),
        ..outcome(ResolveStatus::Fallback)
    };
    let (_, line) = console::resolve_outcome_line("*", &sample);
    let shown = line.split_once(" -- ").expect("message").1;
    assert_eq!(shown.len(), 200);
    assert!(shown.ends_with(".."));
}

#[test]
fn resolve_public_message_limit_handles_multibyte_input_at_the_boundary() {
    for position in 197..=201 {
        let message = format!("{}\u{e9}\u{1f642}{}", "x".repeat(position), "y".repeat(50));
        let sample = Outcome {
            public_message: Some(&message),
            ..outcome(ResolveStatus::Fallback)
        };
        let (_, line) = console::resolve_outcome_line("*", &sample);
        let shown = line.split_once(" -- ").expect("message").1;
        assert_eq!(shown.len(), 200);
        assert!(shown.is_ascii());
        assert!(shown.ends_with(".."));
        assert!(shown.starts_with(&"x".repeat(position.min(198))));
    }
}

#[test]
fn playback_notices_are_ascii_colored_and_restore_the_prompt() {
    for style in [
        Style::plain(),
        Style {
            color: true,
            unicode: true,
        },
    ] {
        let (ux, buffer) = ux(style);
        let counts = Arc::new(Mutex::new(Counts::default()));
        let _guard = ux.use_overlay(Box::new(Counting(Arc::clone(&counts))));
        ux.playback_error(
            "youtube.com",
            &console::PlaybackError::proxy("access_denied"),
        );
        let expected = if style.color {
            format!(
                "> {}12:34:56  {}Problem    {}youtube.com{}{}The video site refused the server's request.\x1b[0m\n> ",
                Color::DarkGray.sgr(),
                Color::Yellow.sgr(),
                Color::Gray.sgr(),
                sp(17),
                Color::White.sgr()
            )
        } else {
            format!(
                "> 12:34:56  Problem    youtube.com{}The video site refused the server's request.\n> ",
                sp(17)
            )
        };
        let text = buffer.text();
        assert_eq!(text, expected);
        assert!(text.is_ascii());
        if style.color {
            assert!(text.ends_with("\x1b[0m\n> "));
        }
        let counts = counts.lock().expect("counts");
        assert_eq!((counts.clear, counts.render), (1, 2));
    }
}

#[test]
fn write_clears_and_restores_the_active_overlay() {
    let (ux, buf) = ux(Style::plain());
    let counts = Arc::new(Mutex::new(Counts::default()));
    let guard = ux.use_overlay(Box::new(Counting(std::sync::Arc::clone(&counts))));
    assert_eq!(counts.lock().map(|c| c.render).unwrap_or(0), 1);

    ux.notice(Color::Gray, "hello");

    let (clear, render) = counts
        .lock()
        .map(|c| (c.clear, c.render))
        .unwrap_or_default();
    assert_eq!((clear, render), (1, 2));
    assert!(buf.text().contains("12:34:56  hello\n"));

    drop(guard);
    let cleared = counts.lock().map(|c| c.clear).unwrap_or(0);
    assert_eq!(cleared, 2);
    ux.notice(Color::Gray, "after");
    assert_eq!(counts.lock().map(|c| c.render).unwrap_or(0), 2);
}

#[test]
fn a_stale_overlay_guard_does_not_remove_a_newer_overlay() {
    let (ux, _) = ux(Style::plain());
    let first = Arc::new(Mutex::new(Counts::default()));
    let second = Arc::new(Mutex::new(Counts::default()));
    let old = ux.use_overlay(Box::new(Counting(first)));
    let _new = ux.use_overlay(Box::new(Counting(std::sync::Arc::clone(&second))));
    drop(old);
    ux.notice(Color::Gray, "x");
    assert_eq!(second.lock().map(|c| c.render).unwrap_or(0), 2);
}

#[test]
fn a_failing_overlay_is_dropped_and_writes_continue() {
    let (ux, buf) = ux(Style::plain());
    let _g = ux.use_overlay(Box::new(Broken));
    ux.notice(Color::Gray, "one");
    ux.notice(Color::Gray, "two");
    assert!(buf.text().contains("12:34:56  one"));
    assert!(buf.text().contains("12:34:56  two"));
}

#[test]
fn notice_levels_use_plain_text_and_colors() {
    let (ux, buf) = ux(Style {
        color: true,
        unicode: true,
    });
    ux.notice(Color::Yellow, "w");
    ux.notice(Color::Red, "e");
    ux.notice(Color::White, "s");
    ux.notice(Color::Red, "f");
    let text = buf.text();
    for (color, body) in [
        (Color::Yellow, "w"),
        (Color::Red, "e"),
        (Color::White, "s"),
        (Color::Red, "f"),
    ] {
        assert!(text.contains(&format!(
            "{}12:34:56  {}{body}\x1b[0m\n",
            Color::DarkGray.sgr(),
            color.sgr()
        )));
    }
    assert!(!text.contains("[warn]"));
    assert!(!text.contains("[err]"));
    assert!(!text.contains('*'));
}

#[test]
fn style_detection_follows_redirect_no_color_and_ascii() {
    assert_eq!(Style::detect(true, None, None), Style::plain());
    assert_eq!(
        Style::detect(false, None, None),
        Style {
            color: true,
            unicode: true
        }
    );
    assert_eq!(
        Style::detect(false, Some(""), None),
        Style {
            color: true,
            unicode: true
        }
    );
    assert_eq!(
        Style::detect(false, Some("1"), None),
        Style {
            color: false,
            unicode: true
        }
    );
    assert_eq!(
        Style::detect(false, None, Some("TRUE")),
        Style {
            color: true,
            unicode: false
        }
    );
    assert_eq!(
        Style::detect(false, None, Some("1")),
        Style {
            color: true,
            unicode: false
        }
    );
    assert_eq!(
        Style::detect(false, None, Some("0")),
        Style {
            color: true,
            unicode: true
        }
    );
}

#[test]
fn every_component_has_a_bracketed_tag() {
    use Component::*;
    for c in [
        Mesh, Ipc, Hosts, Patch, Wrapper, Heartbeat, Relay, Terminal, Codec, Report, Update,
        VrcLog, YtDlp, Shutdown,
    ] {
        let t = c.tag();
        assert!(
            t.starts_with('[') && t.ends_with(']') && t.len() > 2,
            "{c:?}"
        );
    }
    assert_eq!(YtDlp.tag(), "[yt-dlp]");
    assert_eq!(Shutdown.color(), Color::DarkRed);
}

#[test]
fn resolve_outcome_lines_keep_their_columns() {
    let (c, line) = console::resolve_outcome_line("*", &outcome(ResolveStatus::Resolved));
    assert_eq!(c, Color::Green);
    assert_eq!(
        line,
        format!(
            "* [resolve] OK  resolved  youtube.com{}avpro{}1.2s",
            sp(24),
            sp(13)
        )
    );

    let cached = Outcome {
        via_cache: true,
        via_lh_yt: true,
        ..outcome(ResolveStatus::Resolved)
    };
    let (_, line) = console::resolve_outcome_line("*", &cached);
    assert_eq!(
        line,
        format!(
            "* [resolve] OK  cached    youtube.com{} [yt]  avpro{}1.2s",
            sp(17),
            sp(13)
        )
    );

    let failed = Outcome {
        reason: Some("timeout"),
        elapsed: Duration::from_secs(125),
        host: "a-very-long-host-name-that-overflows.example.com",
        ..outcome(ResolveStatus::Failed)
    };
    let (c, line) = console::resolve_outcome_line("*", &failed);
    assert_eq!(c, Color::Red);
    assert_eq!(
        line,
        format!(
            "* [resolve] XX  failed    a-very-long-host-name-that..{}avpro{}2m5s   timeout",
            sp(7),
            sp(13)
        )
    );

    let local = Outcome {
        reason: Some("all_configs_failed"),
        ..outcome(ResolveStatus::Local)
    };
    let (c, line) = console::resolve_outcome_line("*", &local);
    assert_eq!(c, Color::Green);
    assert_eq!(
        line,
        format!(
            "* [resolve] OK  local     youtube.com{}avpro{}1.2s   all_configs_failed",
            sp(24),
            sp(13)
        )
    );

    let (c, line) = console::resolve_outcome_line("*", &outcome(ResolveStatus::Fallback));
    assert_eq!(c, Color::Yellow);
    assert!(line.starts_with("* [resolve] !!  fallback  "));
    let (c, line) = console::resolve_outcome_line(
        "*",
        &Outcome {
            reason: Some(""),
            ..outcome(ResolveStatus::Unexpected)
        },
    );
    assert_eq!(c, Color::DarkGray);
    assert!(line.starts_with("* [resolve] ??  unknown   ") && line.ends_with("1.2s"));
}

#[test]
fn truncate_pad_and_elapsed_match_the_old_console() {
    assert_eq!(console::truncate_pad("abc", 5), "abc  ");
    assert_eq!(console::truncate_pad("abcdef", 5), "abc..");
    assert_eq!(console::truncate_pad("abcdef", 3), "abc");
    assert_eq!(
        console::truncate_pad("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 5),
        "\u{e9}\u{e9}\u{e9}.."
    );
    assert_eq!(console::format_elapsed(Duration::ZERO), "0.0s");
    assert_eq!(
        console::format_elapsed(Duration::from_millis(59_940)),
        "59.9s"
    );
    assert_eq!(console::format_elapsed(Duration::from_secs(60)), "1m0s");
    assert_eq!(
        console::format_elapsed(Duration::from_millis(3_725_900)),
        "62m5s"
    );
}

#[test]
fn phase_divider_centres_the_label_in_sixty_columns() {
    let line = console::phase_divider_line("startup");
    assert_eq!(line.len(), 60);
    assert_eq!(
        line,
        format!("{} startup {}", "-".repeat(25), "-".repeat(26))
    );
    let long = "x".repeat(70);
    assert_eq!(console::phase_divider_line(&long), format!(" {long} "));
}

#[test]
fn banner_pads_labels_and_flags_dev_mode() {
    let paths = [
        ("logs", "C:\\logs".to_owned()),
        ("state root", "C:\\state".to_owned()),
    ];
    let b = Banner {
        version: "2026.10.14.0",
        channel: Some("nightly"),
        sha: "abc1234",
        build_time: "now",
        dev: true,
        paths: &paths,
    };
    let lines: Vec<String> = console::banner_lines(&b)
        .into_iter()
        .map(|(_, l)| l)
        .collect();
    assert_eq!(lines[1], "  VRCResolver 2026.10.14.0 (nightly)");
    assert!(lines[4].starts_with("  debug: on"));
    assert_eq!(lines[6], "  logs:        C:\\logs");
    assert_eq!(lines[7], "  state root:  C:\\state");
    assert_eq!(lines.last().map(String::as_str), Some(""));
    let quiet = Banner {
        channel: None,
        dev: false,
        ..b
    };
    let quiet: Vec<String> = console::banner_lines(&quiet)
        .into_iter()
        .map(|(_, l)| l)
        .collect();
    assert_eq!(quiet[1], "  VRCResolver 2026.10.14.0");
    assert!(!quiet.iter().any(|l| l.contains("debug")));
}
