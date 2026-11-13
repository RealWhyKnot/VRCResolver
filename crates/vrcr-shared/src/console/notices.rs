use super::{Color, Ux};

#[derive(Clone, Copy, Debug)]
pub enum Notice {
    HostsAddFailed,
    HostsRemoveFailed,
    HostsSetupSlow,
    HostsPermissionDeclined,
    HostsSetupLaunchFailed,
    HostsPermissionNeeded,
    HostsSetupIncomplete,
    SecurePlaybackRetry,
    ReportRejected,
    ReportSendFailed,
    SecurePermissionNeeded,
    SecurePermissionDeclined,
    SecureSetupSlow,
    SecureSetupLaunchFailed,
    SecureSetupIncomplete,
    CertificateReadFailed,
    CertificateCreateFailed,
    CertificateSaveFailed,
    SecurePortInvalid,
    CertificateInvalid,
    SecureSetupFailed,
    OldCertificateRemoveFailed,
    OldConnectionRemoveFailed,
    SecureCleanupFailed,
    Disconnected,
    ProtocolError,
    VrchatOpen,
    AlreadyRunning,
    InstanceCheckFailed,
    StartupFailed,
    CloseHandlerFailed,
    Closing,
    RestoreFailed,
    RelayStartFailed,
    LocalPortUnavailable,
    Ready,
    NoSound,
    AudioUnavailable,
    AudioRestored,
}

impl Ux {
    pub fn event(&self, event: Notice) {
        let (color, sentence) = match event {
            Notice::NoSound => (
                Color::Red,
                "VRChat has no sound output. Pick a different default playback device in Windows Sound settings.",
            ),
            Notice::AudioUnavailable => (
                Color::Yellow,
                "VRChat found no sound devices. Videos may fail to load until Windows audio is working again.",
            ),
            Notice::AudioRestored => (Color::Gray, "VRChat's sound devices are back."),

            Notice::HostsAddFailed => (
                Color::Red,
                "VRCResolver couldn't enable video playback in public worlds.",
            ),
            Notice::HostsRemoveFailed => (
                Color::Red,
                "VRCResolver couldn't remove its public-world playback settings.",
            ),
            Notice::HostsSetupSlow => (
                Color::Yellow,
                "Windows is still setting up public-world playback. VRCResolver will carry on without it.",
            ),
            Notice::HostsPermissionDeclined => (
                Color::Yellow,
                "Permission was declined. Videos from this PC may not play in public worlds.",
            ),
            Notice::HostsSetupLaunchFailed => (
                Color::Yellow,
                "Windows couldn't open public-world playback setup.",
            ),
            Notice::HostsPermissionNeeded => (
                Color::Yellow,
                "Windows will ask for permission to let videos from this PC play in public worlds.",
            ),
            Notice::HostsSetupIncomplete => (
                Color::Yellow,
                "Public-world playback setup didn't finish. Videos from this PC may not play in public worlds.",
            ),
            Notice::SecurePlaybackRetry => (
                Color::Yellow,
                "Secure video playback couldn't start. Trying an unsecured local connection.",
            ),
            Notice::ReportRejected => (Color::Yellow, "The server didn't accept the error report."),
            Notice::ReportSendFailed => {
                (Color::Yellow, "VRCResolver couldn't send the error report.")
            }
            Notice::SecurePermissionNeeded => (
                Color::Yellow,
                "Windows will ask for permission to set up secure video playback on this PC.",
            ),
            Notice::SecurePermissionDeclined => (
                Color::Yellow,
                "Permission was declined. Videos from this PC will use an unsecured local connection.",
            ),
            Notice::SecureSetupSlow => (
                Color::Yellow,
                "Secure video setup is taking too long. Videos from this PC will use an unsecured local connection.",
            ),
            Notice::SecureSetupLaunchFailed => (
                Color::Yellow,
                "Windows couldn't open secure video setup. Videos from this PC will use an unsecured local connection.",
            ),
            Notice::SecureSetupIncomplete => (
                Color::Yellow,
                "Secure video setup didn't finish. Videos from this PC will use an unsecured local connection.",
            ),
            Notice::CertificateReadFailed => (
                Color::Yellow,
                "VRCResolver couldn't read its security certificate. Videos from this PC will use an unsecured local connection.",
            ),
            Notice::CertificateCreateFailed => (
                Color::Yellow,
                "VRCResolver couldn't create a security certificate. Videos from this PC will use an unsecured local connection.",
            ),
            Notice::CertificateSaveFailed => (
                Color::Yellow,
                "VRCResolver couldn't save its security certificate. Secure video setup may be needed again next time.",
            ),
            Notice::SecurePortInvalid | Notice::CertificateInvalid | Notice::SecureSetupFailed => {
                (Color::Red, "Secure video setup failed.")
            }
            Notice::OldCertificateRemoveFailed => (
                Color::Yellow,
                "Secure video setup couldn't remove an old security certificate.",
            ),
            Notice::OldConnectionRemoveFailed => (
                Color::Yellow,
                "VRCResolver couldn't remove an old secure video connection.",
            ),
            Notice::SecureCleanupFailed => (
                Color::Yellow,
                "VRCResolver couldn't finish removing its secure video settings.",
            ),
            Notice::Disconnected => (
                Color::Yellow,
                "Lost the connection to the server. Trying again...",
            ),
            Notice::ProtocolError => (
                Color::Yellow,
                "The server couldn't understand a request from VRCResolver.",
            ),
            Notice::VrchatOpen => (
                Color::Gray,
                "VRChat is open. Setup finishes once no video is loading.",
            ),
            Notice::AlreadyRunning => (Color::Yellow, "VRCResolver is already running."),
            Notice::InstanceCheckFailed => (
                Color::Red,
                "VRCResolver couldn't check whether it's already running. Try opening it again.",
            ),
            Notice::StartupFailed => (
                Color::Red,
                "VRCResolver couldn't start. Try opening it again.",
            ),
            Notice::CloseHandlerFailed => (
                Color::Yellow,
                "VRCResolver couldn't watch for this window closing. Close it with the quit command to restore VRChat's video player.",
            ),
            Notice::Closing => (
                Color::Gray,
                "Closing. VRChat's video player is back to normal.",
            ),
            Notice::RestoreFailed => (
                Color::Red,
                "Closing. VRCResolver couldn't restore VRChat's video player. Open VRCResolver again to retry.",
            ),
            Notice::RelayStartFailed => (
                Color::Red,
                "Videos from this PC are unavailable because VRCResolver couldn't start.",
            ),
            Notice::LocalPortUnavailable => (
                Color::Red,
                "Videos from this PC are unavailable because VRCResolver couldn't open a local connection.",
            ),
            Notice::Ready => (
                Color::White,
                "Ready. Videos in VRChat now play through VRCResolver.",
            ),
        };
        self.notice(color, sentence);
    }
}
