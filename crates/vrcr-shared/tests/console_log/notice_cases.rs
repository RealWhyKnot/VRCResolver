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
    "VRCResolver couldn't enable video playback in public worlds."
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
    "Windows is still setting up public-world playback. VRCResolver will carry on without it."
);
event_case!(
    hosts_permission_declined,
    HostsPermissionDeclined,
    Yellow,
    "Permission was declined. Videos from this PC may not play in public worlds."
);
event_case!(
    hosts_setup_launch_failed,
    HostsSetupLaunchFailed,
    Yellow,
    "Windows couldn't open public-world playback setup."
);
event_case!(
    hosts_permission_needed,
    HostsPermissionNeeded,
    Yellow,
    "Windows will ask for permission to let videos from this PC play in public worlds."
);
event_case!(
    hosts_setup_incomplete,
    HostsSetupIncomplete,
    Yellow,
    "Public-world playback setup didn't finish. Videos from this PC may not play in public worlds."
);
event_case!(
    secure_playback_retry,
    SecurePlaybackRetry,
    Yellow,
    "Secure video playback couldn't start. Trying an unsecured local connection."
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
    secure_permission_needed,
    SecurePermissionNeeded,
    Yellow,
    "Windows will ask for permission to set up secure video playback on this PC."
);
event_case!(
    secure_permission_declined,
    SecurePermissionDeclined,
    Yellow,
    "Permission was declined. Videos from this PC will use an unsecured local connection."
);
event_case!(
    secure_setup_slow,
    SecureSetupSlow,
    Yellow,
    "Secure video setup is taking too long. Videos from this PC will use an unsecured local connection."
);
event_case!(
    secure_setup_launch_failed,
    SecureSetupLaunchFailed,
    Yellow,
    "Windows couldn't open secure video setup. Videos from this PC will use an unsecured local connection."
);
event_case!(
    secure_setup_incomplete,
    SecureSetupIncomplete,
    Yellow,
    "Secure video setup didn't finish. Videos from this PC will use an unsecured local connection."
);
event_case!(
    certificate_read_failed,
    CertificateReadFailed,
    Yellow,
    "VRCResolver couldn't read its security certificate. Videos from this PC will use an unsecured local connection."
);
event_case!(
    certificate_create_failed,
    CertificateCreateFailed,
    Yellow,
    "VRCResolver couldn't create a security certificate. Videos from this PC will use an unsecured local connection."
);
event_case!(
    certificate_save_failed,
    CertificateSaveFailed,
    Yellow,
    "VRCResolver couldn't save its security certificate. Secure video setup may be needed again next time."
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
    "Secure video setup couldn't remove an old security certificate."
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
    "VRCResolver couldn't watch for this window closing. Close it with the quit command to restore VRChat's video player."
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
