use vrcr_watchdog::maintenance::update_dialog::{
    self, DialogButton, LATER_BUTTON, SKIP_VERSION_BUTTON, UPDATE_NOW_BUTTON, UpdateDecision,
    decision_for, dialog_copy,
};
use windows_sys::Win32::UI::Controls::{
    TDF_ALLOW_DIALOG_CANCELLATION, TDF_USE_COMMAND_LINKS, TDF_USE_COMMAND_LINKS_NO_ICON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{IDCANCEL, IDCLOSE};

#[test]
fn dialog_copy_names_both_versions_and_three_choices() {
    let copy = dialog_copy("2026.11.24.0", "2026.11.20.0");
    assert_eq!(copy.title, "VRCResolver Update");
    assert_eq!(copy.instruction, "VRCResolver 2026.11.24.0 is available");
    assert_eq!(
        copy.content,
        "You're running 2026.11.20.0. Installing will restart VRCResolver."
    );
    assert_eq!(
        copy.buttons,
        vec![
            DialogButton {
                id: UPDATE_NOW_BUTTON,
                text: "Update now\nDownload and install 2026.11.24.0".to_owned(),
            },
            DialogButton {
                id: LATER_BUTTON,
                text: "Not now\nAsk again next time VRCResolver starts".to_owned(),
            },
            DialogButton {
                id: SKIP_VERSION_BUTTON,
                text:
                    "Don't ask again for this version\nNotify again when a newer release comes out"
                        .to_owned(),
            },
        ]
    );
    assert_eq!(copy.default_button, UPDATE_NOW_BUTTON);
}

#[test]
fn dialog_copy_drops_the_running_version_when_unknown() {
    for current in ["", "   "] {
        assert_eq!(
            dialog_copy("2026.11.24.0", current).content,
            "Installing will restart VRCResolver."
        );
    }
}

#[test]
fn dialog_uses_cancellable_command_links() {
    assert_eq!(
        update_dialog::FLAGS,
        TDF_USE_COMMAND_LINKS | TDF_ALLOW_DIALOG_CANCELLATION
    );
    assert_eq!(update_dialog::FLAGS & TDF_USE_COMMAND_LINKS_NO_ICON, 0);
}

#[test]
fn button_ids_map_to_decisions_and_cancel_means_later() {
    let ids = [UPDATE_NOW_BUTTON, LATER_BUTTON, SKIP_VERSION_BUTTON];
    assert_eq!(
        ids.len(),
        ids.iter().collect::<std::collections::HashSet<_>>().len()
    );
    assert!(ids.iter().all(|id| *id != IDCANCEL && *id != IDCLOSE));
    assert_eq!(decision_for(UPDATE_NOW_BUTTON), UpdateDecision::UpdateNow);
    assert_eq!(decision_for(LATER_BUTTON), UpdateDecision::Later);
    assert_eq!(
        decision_for(SKIP_VERSION_BUTTON),
        UpdateDecision::SkipVersion
    );
    for other in [IDCANCEL, IDCLOSE, 0, -1, 9999] {
        assert_eq!(decision_for(other), UpdateDecision::Later);
    }
}
