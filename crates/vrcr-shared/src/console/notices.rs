use super::{Color, Ux};

#[derive(Clone, Copy, Debug)]
pub enum Notice {
    HostsAddFailed,
    HostsRemoveFailed,
    HostsSetupSlow,
    HostsPermissionDeclined,
    HostsSetupLaunchFailed,
    HostsSetupIncomplete,
    PublicVideoSetupMissing,
    VideoWrapperMissing,
    UntrustedUrlBlocked,
    SecurePlaybackRetry,
    ReportRejected,
    ReportSendFailed,
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
                "VRCResolver couldn't set up videos for public worlds. Type repair to be asked again.",
            ),
            Notice::HostsRemoveFailed => (
                Color::Red,
                "VRCResolver couldn't remove its public-world playback settings.",
            ),
            Notice::HostsSetupSlow => (
                Color::Yellow,
                "Windows took too long to set up videos for public worlds. Type repair to be asked again.",
            ),
            Notice::HostsPermissionDeclined => (
                Color::Yellow,
                "Permission for videos in public worlds was declined. Type repair to be asked again.",
            ),
            Notice::HostsSetupLaunchFailed => (
                Color::Yellow,
                "Windows couldn't open video setup for public worlds. Type repair to be asked again.",
            ),
            Notice::HostsSetupIncomplete => (
                Color::Yellow,
                "Video setup for public worlds didn't finish. Type repair to be asked again.",
            ),
            Notice::PublicVideoSetupMissing => (
                Color::Yellow,
                "Type repair to finish setting up videos for public worlds.",
            ),
            Notice::VideoWrapperMissing => (
                Color::Yellow,
                "Type repair to reconnect VRCResolver to VRChat's video player.",
            ),
            Notice::UntrustedUrlBlocked => (
                Color::Yellow,
                "A video link may be blocked. Enable Allow Untrusted URLs in VRChat Settings > Comfort & Safety.",
            ),
            Notice::SecurePlaybackRetry => (
                Color::Yellow,
                "Secure video couldn't start. Trying video without a secure connection.",
            ),
            Notice::ReportRejected => (Color::Yellow, "The server didn't accept the error report."),
            Notice::ReportSendFailed => {
                (Color::Yellow, "VRCResolver couldn't send the error report.")
            }
            Notice::SecurePermissionDeclined => (
                Color::Yellow,
                "Permission for secure video was declined. Type repair to be asked again.",
            ),
            Notice::SecureSetupSlow => (
                Color::Yellow,
                "Windows took too long to set up secure video. Type repair to be asked again.",
            ),
            Notice::SecureSetupLaunchFailed => (
                Color::Yellow,
                "Windows couldn't open secure video setup. Type repair to be asked again.",
            ),
            Notice::SecureSetupIncomplete => (
                Color::Yellow,
                "Secure video setup didn't finish. Type repair to be asked again.",
            ),
            Notice::CertificateReadFailed => (
                Color::Yellow,
                "VRCResolver couldn't read its secure video setup. Type repair to be asked again.",
            ),
            Notice::CertificateCreateFailed => (
                Color::Yellow,
                "VRCResolver couldn't prepare secure video playback. Type repair to be asked again.",
            ),
            Notice::CertificateSaveFailed => (
                Color::Yellow,
                "VRCResolver couldn't save its secure video setup. Type repair to be asked again.",
            ),
            Notice::SecurePortInvalid | Notice::CertificateInvalid | Notice::SecureSetupFailed => {
                (Color::Red, "Secure video setup failed.")
            }
            Notice::OldCertificateRemoveFailed => (
                Color::Yellow,
                "VRCResolver couldn't remove an old security certificate.",
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
                "Next time you open VRCResolver, it will put VRChat's video player back.",
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
