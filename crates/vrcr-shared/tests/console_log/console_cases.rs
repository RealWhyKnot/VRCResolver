use super::*;

#[test]
fn write_clears_and_restores_the_active_overlay() {
    let (ux, buf) = ux(Style::plain());
    let counts = Arc::new(Mutex::new(Counts::default()));
    let guard = ux.use_overlay(Box::new(Counting(std::sync::Arc::clone(&counts))));
    assert_eq!(counts.lock().map(|c| c.render).unwrap_or(0), 1);

    ux.write(Component::Terminal, "hello");

    let (clear, render) = counts
        .lock()
        .map(|c| (c.clear, c.render))
        .unwrap_or_default();
    assert_eq!((clear, render), (1, 2));
    assert!(buf.text().contains("[12:34:56] * [terminal] hello\n"));

    drop(guard);
    let cleared = counts.lock().map(|c| c.clear).unwrap_or(0);
    assert_eq!(cleared, 2);
    ux.write(Component::Mesh, "after");
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
    ux.write(Component::Ipc, "x");
    assert_eq!(second.lock().map(|c| c.render).unwrap_or(0), 2);
}

#[test]
fn a_failing_overlay_is_dropped_and_writes_continue() {
    let (ux, buf) = ux(Style::plain());
    let _g = ux.use_overlay(Box::new(Broken));
    ux.write(Component::Relay, "one");
    ux.write(Component::Relay, "two");
    assert!(buf.text().contains("[relay] one"));
    assert!(buf.text().contains("[relay] two"));
}

#[test]
fn levels_use_their_tags_and_colors() {
    let (ux, buf) = ux(Style {
        color: true,
        unicode: true,
    });
    ux.warn(Component::Hosts, "w");
    ux.error(Component::Patch, "e");
    ux.success(Component::Update, "s");
    ux.fatal("f");
    let text = buf.text();
    assert!(text.contains(&format!(
        "{}[12:34:56] \u{2022} [hosts][warn] w\x1b[0m",
        Color::Yellow.sgr()
    )));
    assert!(text.contains(&format!(
        "{}[12:34:56] \u{2022} [patch][err] e\x1b[0m",
        Color::Red.sgr()
    )));
    assert!(text.contains(&format!(
        "{}[12:34:56] \u{2022} [update] s\x1b[0m",
        Color::Green.sgr()
    )));
    assert!(text.contains("\u{2022} [FATAL] f"));
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
