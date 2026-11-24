use super::*;

macro_rules! event_case {
    ($name:ident, $event:ident, $color:ident, $text:literal) => {
        #[test]
        fn $name() {
            let (plain, output) = ux(Style::plain());
            plain.event(console::Notice::$event);
            assert_eq!(output.text(), concat!("12:34:56  ", $text, "\n"));
            let (colored, output) = ux(Style {
                color: true,
                unicode: false,
            });
            colored.event(console::Notice::$event);
            assert_eq!(
                output.text(),
                format!(
                    "{}12:34:56  {}{}\x1b[0m\n",
                    Color::DarkGray.sgr(),
                    Color::$color.sgr(),
                    $text
                )
            );
        }
    };
}

event_case!(
    hosts_add_failed,
    HostsAddFailed,
    Red,
    "VRCResolver couldn't set up videos for public worlds. Type repair to be asked again."
);
event_case!(
    hosts_remove_failed,
    HostsRemoveFailed,
    Red,
    "VRCResolver couldn't remove its public-world playback settings."
);
event_case!(
    hosts_setup_slow,
    HostsSetupSlow,
    Yellow,
    "Windows took too long to set up videos for public worlds. Type repair to be asked again."
);
event_case!(
    hosts_permission_declined,
    HostsPermissionDeclined,
    Yellow,
    "Permission for videos in public worlds was declined. Type repair to be asked again."
);
event_case!(
    hosts_setup_launch_failed,
    HostsSetupLaunchFailed,
    Yellow,
    "Windows couldn't open video setup for public worlds. Type repair to be asked again."
);
event_case!(
    hosts_setup_incomplete,
    HostsSetupIncomplete,
    Yellow,
    "Video setup for public worlds didn't finish. Type repair to be asked again."
);
event_case!(
    secure_playback_retry,
    SecurePlaybackRetry,
    Yellow,
    "Secure video couldn't start. Trying video without a secure connection."
);
event_case!(
    report_rejected,
    ReportRejected,
    Yellow,
    "The server didn't accept the error report."
);
event_case!(
    report_send_failed,
    ReportSendFailed,
    Yellow,
    "VRCResolver couldn't send the error report."
);
event_case!(
    secure_permission_declined,
    SecurePermissionDeclined,
    Yellow,
    "Permission for secure video was declined. Type repair to be asked again."
);
event_case!(
    secure_setup_slow,
    SecureSetupSlow,
    Yellow,
    "Windows took too long to set up secure video. Type repair to be asked again."
);
event_case!(
    secure_setup_launch_failed,
    SecureSetupLaunchFailed,
    Yellow,
    "Windows couldn't open secure video setup. Type repair to be asked again."
);
event_case!(
    secure_setup_incomplete,
    SecureSetupIncomplete,
    Yellow,
    "Secure video setup didn't finish. Type repair to be asked again."
);
event_case!(
    certificate_read_failed,
    CertificateReadFailed,
    Yellow,
    "VRCResolver couldn't read its secure video setup. Type repair to be asked again."
);
event_case!(
    certificate_create_failed,
    CertificateCreateFailed,
    Yellow,
    "VRCResolver couldn't prepare secure video playback. Type repair to be asked again."
);
event_case!(
    certificate_save_failed,
    CertificateSaveFailed,
    Yellow,
    "VRCResolver couldn't save its secure video setup. Type repair to be asked again."
);
event_case!(
    secure_port_invalid,
    SecurePortInvalid,
    Red,
    "Secure video setup failed."
);
event_case!(
    certificate_invalid,
    CertificateInvalid,
    Red,
    "Secure video setup failed."
);
event_case!(
    secure_setup_failed,
    SecureSetupFailed,
    Red,
    "Secure video setup failed."
);
event_case!(
    old_certificate_remove_failed,
    OldCertificateRemoveFailed,
    Yellow,
    "VRCResolver couldn't remove an old security certificate."
);
event_case!(
    old_connection_remove_failed,
    OldConnectionRemoveFailed,
    Yellow,
    "VRCResolver couldn't remove an old secure video connection."
);
event_case!(
    secure_cleanup_failed,
    SecureCleanupFailed,
    Yellow,
    "VRCResolver couldn't finish removing its secure video settings."
);
event_case!(
    disconnected,
    Disconnected,
    Yellow,
    "Lost the connection to the server. Trying again..."
);
event_case!(
    protocol_error,
    ProtocolError,
    Yellow,
    "The server couldn't understand a request from VRCResolver."
);
event_case!(
    vrchat_open,
    VrchatOpen,
    Gray,
    "VRChat is open. Setup finishes once no video is loading."
);
event_case!(
    already_running,
    AlreadyRunning,
    Yellow,
    "VRCResolver is already running."
);
event_case!(
    instance_check_failed,
    InstanceCheckFailed,
    Red,
    "VRCResolver couldn't check whether it's already running. Try opening it again."
);
event_case!(
    startup_failed,
    StartupFailed,
    Red,
    "VRCResolver couldn't start. Try opening it again."
);
event_case!(
    close_handler_failed,
    CloseHandlerFailed,
    Yellow,
    "Next time you open VRCResolver, it will put VRChat's video player back."
);
event_case!(
    closing,
    Closing,
    Gray,
    "Closing. VRChat's video player is back to normal."
);
event_case!(
    restore_failed,
    RestoreFailed,
    Red,
    "Closing. VRCResolver couldn't restore VRChat's video player. Open VRCResolver again to retry."
);
event_case!(
    relay_start_failed,
    RelayStartFailed,
    Red,
    "Videos from this PC are unavailable because VRCResolver couldn't start."
);
event_case!(
    local_port_unavailable,
    LocalPortUnavailable,
    Red,
    "Videos from this PC are unavailable because VRCResolver couldn't open a local connection."
);
event_case!(
    ready,
    Ready,
    White,
    "Ready. Videos in VRChat now play through VRCResolver."
);

event_case!(
    nosound,
    NoSound,
    Red,
    "VRChat has no sound output. Pick a different default playback device in Windows Sound settings."
);
event_case!(
    audiounavailable,
    AudioUnavailable,
    Yellow,
    "VRChat found no sound devices. Videos may fail to load until Windows audio is working again."
);
event_case!(
    audiorestored,
    AudioRestored,
    Gray,
    "VRChat's sound devices are back."
);

const MISSING_UPDATER: &str =
    "Version 2026.11.24.0 is available, but the updater is missing. Download the new version from";
const RELEASE_LINK: &str = "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v2026.11.24.0";

#[test]
fn notice_with_link_keeps_the_whole_link_on_one_line_without_a_width() {
    let (ux, output) = ux(Style::plain());
    ux.notice_with_link(Color::Yellow, MISSING_UPDATER, RELEASE_LINK);
    assert_eq!(
        output.text(),
        format!("12:34:56  {MISSING_UPDATER} {RELEASE_LINK}\n")
    );
}

#[test]
fn notice_with_link_colors_the_link_like_the_sentence() {
    let (ux, output) = ux(Style {
        color: true,
        unicode: false,
    });
    ux.notice_with_link(Color::Yellow, MISSING_UPDATER, RELEASE_LINK);
    assert_eq!(
        output.text(),
        format!(
            "{}12:34:56  {}{MISSING_UPDATER} {RELEASE_LINK}\x1b[0m\n",
            Color::DarkGray.sgr(),
            Color::Yellow.sgr()
        )
    );
}

#[test]
fn notice_with_link_never_splits_or_redacts_the_link_at_any_width() {
    thread_local! { static WIDTH: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
    let link_len = RELEASE_LINK.len();
    for width in (12..=200).map(Some).chain([None]) {
        WIDTH.set(width);
        let (ux, output) = ux(Style::plain());
        ux.with_width(|| WIDTH.get()).notice_with_link(
            Color::Yellow,
            MISSING_UPDATER,
            RELEASE_LINK,
        );
        let text = output.text();
        let lines: Vec<&str> = text.lines().collect();
        let holding: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(RELEASE_LINK))
            .collect();
        assert_eq!(holding.len(), 1, "{width:?}: {text}");
        let link_line = holding[0];
        assert!(link_line.ends_with(RELEASE_LINK), "{width:?}: {text}");
        assert_eq!(lines.last(), Some(link_line), "{width:?}: {text}");
        if let Some(width) = width {
            let room = width - 1;
            for line in &lines {
                if line != link_line {
                    assert!(line.len() <= room, "{width}: {text}");
                }
            }
            if link_len + 10 <= room {
                assert!(link_line.len() <= room, "{width}: {text}");
            } else {
                assert_eq!(*link_line, RELEASE_LINK, "{width}: {text}");
            }
        }
        let words: Vec<&str> = text.split_whitespace().skip(1).collect();
        let mut expected: Vec<&str> = MISSING_UPDATER.split_whitespace().collect();
        expected.push(RELEASE_LINK);
        if width.is_none_or(|width| width > 30) {
            assert_eq!(words, expected, "{width:?}: {text}");
        }
    }
}

#[test]
fn notice_with_link_wraps_the_link_under_the_sentence_column_at_80_columns() {
    thread_local! { static WIDTH: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
    let short_link = "https://github.com/RealWhyKnot/VRCResolver/releases";
    WIDTH.set(Some(80));
    let (ux, output) = ux(Style::plain());
    ux.with_width(|| WIDTH.get())
        .notice_with_link(Color::Yellow, MISSING_UPDATER, short_link);
    let text = output.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.last(),
        Some(&format!("{}{short_link}", sp(10)).as_str())
    );
}

#[test]
fn notice_with_link_drops_control_characters_from_the_link() {
    let (ux, output) = ux(Style::plain());
    ux.notice_with_link(
        Color::Yellow,
        "Download it from",
        "https://example.com/\x1b]8;;evil\x07a b",
    );
    assert_eq!(
        output.text(),
        "12:34:56  Download it from https://example.com/]8;;evilab\n"
    );
}
