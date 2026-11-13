use super::*;
use console::VideoStatus;

fn row(status: VideoStatus, sentence: &str) -> String {
    format!(
        "12:34:56  {:<11}{:<28}{sentence}\n",
        status.word(),
        "youtube.com"
    )
}

#[test]
fn playing_reports_delivered_quality_and_elapsed_seconds() {
    let (ux, buf) = ux(Style::plain());
    let mut sample = outcome(ResolveStatus::Resolved);
    sample.height = Some(1080);
    sample.elapsed = Duration::from_millis(3200);
    ux.resolve_outcome(&sample);
    assert_eq!(
        buf.text(),
        row(VideoStatus::Playing, "1080p, ready in 3.2s")
    );
}

#[test]
fn playing_without_quality_omits_the_comma() {
    let (ux, buf) = ux(Style::plain());
    ux.resolve_outcome(&outcome(ResolveStatus::Local));
    assert_eq!(buf.text(), row(VideoStatus::Playing, "ready in 1.2s"));
}

#[test]
fn skipped_uses_the_sanitized_server_sentence_once() {
    let (ux, buf) = ux(Style::plain());
    ux.resolve_outcome(&Outcome {
        public_message: Some("\x1b\x1b[31mThe\tvideo\n is private.\x1b[0m\u{2603}"),
        ..outcome(ResolveStatus::Fallback)
    });
    assert_eq!(
        buf.text(),
        row(
            VideoStatus::Skipped,
            "The video is private. VRChat will try it on its own."
        )
    );
}

macro_rules! fallback_case {
    ($name:ident, $reason:literal, $why:literal) => {
        #[test]
        fn $name() {
            for message in [None, Some(""), Some(" \t")] {
                let (ux, buf) = ux(Style::plain());
                ux.skipped("youtube.com", $reason, message);
                assert_eq!(
                    buf.text(),
                    row(
                        VideoStatus::Skipped,
                        concat!($why, " VRChat will try it on its own.")
                    )
                );
            }
        }
    };
}

fallback_case!(
    unreachable,
    "server_unreachable",
    "Couldn't reach the server."
);
fallback_case!(
    paused,
    "resolver_paused",
    "VRCResolver is paused after repeated video failures."
);
fallback_case!(
    prior_failure,
    "prior_load_failure",
    "VRChat couldn't play this video through VRCResolver earlier."
);
fallback_case!(
    client_deadline,
    "client_deadline_exceeded",
    "VRCResolver couldn't get this video in time."
);
fallback_case!(
    client_internal,
    "internal_error",
    "VRCResolver ran into a problem getting this video."
);
fallback_case!(
    client_rate_limit,
    "rate_limited",
    "The server asked VRCResolver to slow down."
);
fallback_case!(
    client_protocol,
    "protocol_error",
    "The server couldn't understand a request from VRCResolver."
);

#[test]
fn client_fallback_reasons_never_use_the_generic_server_sentence() {
    use vrcr_shared::wire::fallback;
    for reason in [
        "resolver_paused",
        "prior_load_failure",
        fallback::SERVER_UNREACHABLE,
        fallback::CLIENT_DEADLINE_EXCEEDED,
        fallback::INTERNAL_ERROR,
        fallback::RATE_LIMITED,
        fallback::PROTOCOL_ERROR,
    ] {
        assert_ne!(
            console::fallback_sentence(reason),
            console::fallback_sentence("unknown"),
            "{reason}"
        );
    }
}

#[test]
fn failed_resolves_remain_yellow_skipped_rows() {
    let (ux, buf) = ux(Style {
        color: true,
        unicode: false,
    });
    ux.resolve_outcome(&Outcome {
        reason: Some("client_deadline_exceeded"),
        ..outcome(ResolveStatus::Failed)
    });
    assert!(buf.text().contains("\x1b[93mSkipped    "));
    assert!(!buf.text().contains("Failed"));
    assert!(
        buf.text().contains(
            "VRCResolver couldn't get this video in time. VRChat will try it on its own."
        )
    );
}
fallback_case!(
    unity,
    "unity_unsupported_format",
    "This world uses VRChat's Unity player, which can't play this video. Worlds with the AVPro player can."
);
fallback_case!(
    missing,
    "content_not_found",
    "The video was removed or doesn't exist."
);
fallback_case!(
    recent_failure,
    "validation_recently_failed",
    "The server couldn't get this video a moment ago."
);
fallback_case!(
    deadline,
    "resolve_cost_exceeds_deadline",
    "The server couldn't get this video in time."
);
fallback_case!(
    all_failed,
    "all_configs_failed",
    "The server couldn't get this video."
);
fallback_case!(
    unknown_failure,
    "unknown",
    "The server couldn't get this video."
);

macro_rules! video_case {
    ($name:ident, $status:ident, $text:literal) => {
        #[test]
        fn $name() {
            let (ux, buf) = ux(Style::plain());
            ux.video(VideoStatus::$status, "youtube.com", $text);
            assert_eq!(buf.text(), row(VideoStatus::$status, $text));
        }
    };
}

video_case!(bundled_failed, Failed, "VRChat couldn't play it either.");
video_case!(
    silent_stall,
    Stalled,
    "The video stopped loading in VRChat."
);
video_case!(load_failure, Failed, "VRChat couldn't load the video.");

#[test]
fn each_status_has_its_own_color_and_sentences_have_the_required_color() {
    for (status, color) in [
        (VideoStatus::Playing, Color::Green),
        (VideoStatus::Skipped, Color::Yellow),
        (VideoStatus::Stalled, Color::Yellow),
        (VideoStatus::Problem, Color::Yellow),
        (VideoStatus::Failed, Color::Red),
    ] {
        let (ux, buf) = ux(Style {
            color: true,
            unicode: true,
        });
        ux.video(status, "youtube.com", "Sentence.");
        let sentence_color = if status == VideoStatus::Playing {
            Color::Gray
        } else {
            Color::White
        };
        let expected = format!(
            "{}12:34:56  {}{:<11}{}{:<28}{}Sentence.\x1b[0m\n",
            Color::DarkGray.sgr(),
            color.sgr(),
            status.word(),
            Color::Gray.sgr(),
            "youtube.com",
            if sentence_color == Color::Gray {
                ""
            } else {
                sentence_color.sgr()
            }
        );
        assert_eq!(buf.text(), expected);
    }
}

macro_rules! notice_case {
    ($name:ident, $color:ident, $text:literal) => {
        #[test]
        fn $name() {
            let (ux, buf) = ux(Style::plain());
            ux.notice(Color::$color, $text);
            assert_eq!(buf.text(), concat!("12:34:56  ", $text, "\n"));
        }
    };
}

notice_case!(
    vrchat_open,
    Gray,
    "VRChat is open. Setup finishes once no video is loading."
);
notice_case!(
    ready,
    White,
    "Ready. Videos in VRChat now play through VRCResolver."
);
notice_case!(first_connection, Gray, "Connected to the server.");
notice_case!(
    disconnected,
    Yellow,
    "Lost the connection to the server. Trying again..."
);
notice_case!(reconnected, Gray, "Connected again after 2m 27s.");
notice_case!(
    rate_limit,
    Yellow,
    "The server asked VRCResolver to slow down. Waiting 5s."
);
notice_case!(
    update,
    White,
    "Version 2026.11.13 is available. Type update to install it."
);
notice_case!(
    shutdown,
    Gray,
    "Closing. VRChat's video player is back to normal."
);

#[test]
fn resolver_health_notices() {
    let (ux, buf) = ux(Style::plain());
    ux.resolver_paused();
    ux.resolver_restored();
    assert_eq!(
        buf.text(),
        "12:34:56  Too many videos failed in a row. VRChat plays videos on its own until the server is back.\n12:34:56  VRCResolver is playing videos again.\n"
    );
}

#[test]
fn outage_durations_use_whole_seconds_minutes_and_hours() {
    for (seconds, expected) in [(45, "45s"), (147, "2m 27s"), (3780, "1h 3m")] {
        assert_eq!(
            console::connection_duration(Duration::from_secs(seconds)),
            expected
        );
    }
}

#[test]
fn banner_keeps_build_details_in_the_log() {
    let dir = TempDir::new().expect("valid console fixture");
    let log = Log::to_file(&dir.path().join("console.log"));
    let (ux, buf) = ux(Style::plain());
    ux.tee_to(log);
    ux.banner(&Banner {
        version: "2026.11.13",
        channel: None,
        sha: "abc123",
        build_time: "now",
        dev: true,
        paths: &[("logs", "secret-path".to_owned())],
    });
    assert_eq!(
        buf.text(),
        "VRCResolver 2026.11.13\nType help for commands.\nDebug build: timings and relay detail go to the log.\n\n"
    );
    let text = fs::read_to_string(dir.path().join("console.log")).expect("valid console fixture");
    assert!(text.contains("sha abc123  built now"));
    assert!(text.contains("secret-path"));
}

#[test]
fn widths_preserve_all_sentence_characters_and_column_alignment() {
    type Width = fn() -> Option<usize>;
    let widths: [(Option<usize>, Width); 10] = [
        (Some(20), || Some(20)),
        (Some(40), || Some(40)),
        (Some(60), || Some(60)),
        (Some(69), || Some(69)),
        (Some(70), || Some(70)),
        (Some(80), || Some(80)),
        (Some(120), || Some(120)),
        (Some(160), || Some(160)),
        (Some(300), || Some(300)),
        (None, || None),
    ];
    for (width, get_width) in widths {
        for length in [11, 27, 28, 300] {
            let host = "a".repeat(length);
            let displayed = if length >= 27 {
                format!("{}...", "a".repeat(24))
            } else {
                host.clone()
            };
            for message in ["word ".repeat(40), "x".repeat(200), "x".repeat(2000)] {
                let clean = vrcr_shared::logutil::public_message(&message);
                let sentence = format!("{clean} VRChat will try it on its own.");
                let (ux, buf) = ux(Style::plain());
                let ux = ux.with_width(get_width);
                ux.resolve_outcome(&Outcome {
                    host: &host,
                    public_message: Some(&message),
                    ..outcome(ResolveStatus::Fallback)
                });
                let text = buf.text();
                let lines: Vec<_> = text.lines().collect();
                let host_width = width.map_or(28, |w| w.saturating_sub(42).min(28));
                let displayed = if host_width == 0 {
                    String::new()
                } else if displayed.len() >= host_width {
                    format!("{}...", &displayed[..host_width - 4])
                } else {
                    displayed.clone()
                };
                let narrow = width == Some(20);
                let indent = if narrow { 10 } else { 21 + host_width };
                let mut body = if narrow {
                    String::new()
                } else {
                    lines[0][21..].to_owned()
                };
                let mut sentence_lines = Vec::new();
                if !narrow {
                    sentence_lines.push(&lines[0][indent..]);
                }
                for line in &lines[1..] {
                    assert!(line.starts_with(&sp(indent)), "{width:?}: {line:?}");
                    body.push_str(&line[indent..]);
                    sentence_lines.push(&line[indent..]);
                }
                if message.starts_with("word ") {
                    assert_eq!(sentence_lines.join(" "), sentence, "{width:?}: {text}");
                }
                let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
                let expected: String = format!("{displayed}{sentence}")
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                assert_eq!(body, expected, "width={width:?}, host={length}");
                assert!(lines[0].starts_with("12:34:56  Skipped"));
                assert_eq!(text.matches("12:34:56").count(), 1);
                assert_eq!(text.matches("Skipped").count(), 1);
                if narrow {
                    assert_eq!(lines[0], "12:34:56  Skipped");
                } else {
                    assert_eq!(&lines[0][21..21 + displayed.len()], displayed);
                    assert!(
                        lines[0].len() > indent,
                        "sentence starts on the event row: {text}"
                    );
                }
                if let Some(width) = width {
                    assert!(lines.iter().all(|line| line.len() < width), "{text}");
                } else {
                    assert_eq!(lines.len(), 1);
                }
            }
        }
    }
}

#[test]
fn redirected_and_no_color_output_is_ascii_without_terminal_escapes() {
    for style in [
        Style::detect(true, None, None),
        Style::detect(false, Some("1"), None),
    ] {
        let (ux, buf) = ux(style);
        ux.video(
            VideoStatus::Failed,
            "https://youtube.com/private?token=x",
            "\x1b[31mHello\x1b[0m \u{2603}\r\nworld.",
        );
        assert_eq!(buf.text(), row(VideoStatus::Failed, "Hello world."));
        assert!(buf.text().is_ascii());
        assert!(!buf.text().contains('\x1b'));
    }
}

#[test]
fn all_video_statuses_share_one_header_row_and_hanging_sentence_columns() {
    type Width = fn() -> Option<usize>;
    let widths: [(Option<usize>, Width); 6] = [
        (Some(40), || Some(40)),
        (Some(60), || Some(60)),
        (Some(69), || Some(69)),
        (Some(70), || Some(70)),
        (Some(120), || Some(120)),
        (None, || None),
    ];
    let sentence = "The video is private. VRChat will try it on its own.";
    for (width, get_width) in widths {
        let column = match width {
            Some(40) => 21,
            Some(60) => 39,
            Some(69) => 48,
            _ => 49,
        };
        for status in [
            VideoStatus::Playing,
            VideoStatus::Skipped,
            VideoStatus::Stalled,
            VideoStatus::Problem,
            VideoStatus::Failed,
        ] {
            let (ux, buf) = ux(Style::plain());
            ux.with_width(get_width)
                .video(status, "youtube.com", sentence);
            let text = buf.text();
            let lines: Vec<_> = text.lines().collect();
            assert!(lines[0].starts_with(&format!("12:34:56  {:<11}", status.word())));
            if column > 21 {
                assert_eq!(lines[0][21..column].trim(), "youtube.com");
            }
            let words: Vec<_> = lines
                .iter()
                .enumerate()
                .flat_map(|(i, line)| {
                    if let Some(width) = width {
                        assert!(line.len() < width, "{text}");
                    }
                    if i > 0 {
                        assert!(line.starts_with(&sp(column)), "{text}");
                    }
                    line[column..].split_whitespace()
                })
                .collect();
            assert_eq!(words, sentence.split_whitespace().collect::<Vec<_>>());
            assert_eq!(text.matches("12:34:56").count(), 1);
            if width.is_none() {
                assert_eq!(lines.len(), 1);
            }
        }
    }
}
