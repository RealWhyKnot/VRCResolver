use std::ptr::{self, null_mut};

use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Component, Ux};
use windows_sys::Win32::Foundation::{HWND, LPARAM, S_OK, WPARAM};
use windows_sys::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_FLAGS, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TD_INFORMATION_ICON,
    TDF_ALLOW_DIALOG_CANCELLATION, TDF_CALLBACK_TIMER, TDF_USE_COMMAND_LINKS, TDM_CLICK_BUTTON,
    TDN_TIMER, TaskDialogIndirect,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows_sys::core::HRESULT;

pub const TITLE: &str = "VRCResolver Update";
pub const UPDATE_NOW_BUTTON: i32 = 1001;
pub const LATER_BUTTON: i32 = 1002;
pub const SKIP_VERSION_BUTTON: i32 = 1003;
pub const FLAGS: TASKDIALOG_FLAGS = TDF_USE_COMMAND_LINKS | TDF_ALLOW_DIALOG_CANCELLATION;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateDecision {
    UpdateNow,
    Later,
    SkipVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogButton {
    pub id: i32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogCopy {
    pub title: String,
    pub instruction: String,
    pub content: String,
    pub buttons: Vec<DialogButton>,
    pub default_button: i32,
}

pub fn dialog_copy(version: &str, current: &str) -> DialogCopy {
    let current = current.trim();
    let content = if current.is_empty() {
        "Installing will restart VRCResolver.".to_owned()
    } else {
        format!("You're running {current}. Installing will restart VRCResolver.")
    };
    let button = |id: i32, text: String| DialogButton { id, text };
    DialogCopy {
        title: TITLE.to_owned(),
        instruction: format!("VRCResolver {version} is available"),
        content,
        buttons: vec![
            button(
                UPDATE_NOW_BUTTON,
                format!("Update now\nDownload and install {version}"),
            ),
            button(
                LATER_BUTTON,
                "Not now\nAsk again next time VRCResolver starts".to_owned(),
            ),
            button(
                SKIP_VERSION_BUTTON,
                "Don't ask again for this version\nNotify again when a newer release comes out"
                    .to_owned(),
            ),
        ],
        default_button: UPDATE_NOW_BUTTON,
    }
}

pub fn decision_for(button: i32) -> UpdateDecision {
    match button {
        UPDATE_NOW_BUTTON => UpdateDecision::UpdateNow,
        SKIP_VERSION_BUTTON => UpdateDecision::SkipVersion,
        _ => UpdateDecision::Later,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub fn ask(version: &str, current: &str, closing: &CancellationToken, ux: &Ux) -> UpdateDecision {
    if closing.is_cancelled() {
        return UpdateDecision::Later;
    }
    let copy = dialog_copy(version, current);
    let title = wide(&copy.title);
    let instruction = wide(&copy.instruction);
    let content = wide(&copy.content);
    let labels: Vec<Vec<u16>> = copy
        .buttons
        .iter()
        .map(|button| wide(&button.text))
        .collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = copy
        .buttons
        .iter()
        .zip(&labels)
        .map(|(button, label)| TASKDIALOG_BUTTON {
            nButtonID: button.id,
            pszButtonText: label.as_ptr(),
        })
        .collect();
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(std::mem::size_of::<TASKDIALOGCONFIG>())
            .expect("dialog config size fits a u32"),
        dwFlags: FLAGS | TDF_CALLBACK_TIMER,
        pszWindowTitle: title.as_ptr(),
        pszMainInstruction: instruction.as_ptr(),
        pszContent: content.as_ptr(),
        cButtons: u32::try_from(buttons.len()).expect("three buttons fit a u32"),
        pButtons: buttons.as_ptr(),
        nDefaultButton: copy.default_button,
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: TD_INFORMATION_ICON,
        },
        pfCallback: Some(dialog_callback),
        lpCallbackData: ptr::from_ref(closing).expose_provenance().cast_signed(),
        ..TASKDIALOGCONFIG::default()
    };
    let mut button = 0;
    // SAFETY: The config, NUL-terminated strings, button array, token and button output live until this modal call returns.
    let result = unsafe { TaskDialogIndirect(&config, &mut button, null_mut(), null_mut()) };
    if result < 0 {
        ux.log_warn(
            Component::Update,
            &format!(
                "Could not open the update dialog: 0x{:08X}",
                result.cast_unsigned()
            ),
        );
        return UpdateDecision::Later;
    }
    if closing.is_cancelled() {
        return UpdateDecision::Later;
    }
    decision_for(button)
}

unsafe extern "system" fn dialog_callback(
    window: HWND,
    notification: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
    callback_data: isize,
) -> HRESULT {
    // SAFETY: TaskDialogIndirect passes back the exposed pointer to the token that outlives the modal call.
    let closing = unsafe {
        &*ptr::with_exposed_provenance::<CancellationToken>(callback_data.cast_unsigned())
    };
    if notification.cast_signed() == TDN_TIMER && closing.is_cancelled() {
        let later = WPARAM::try_from(LATER_BUTTON).expect("button ID fits WPARAM");
        // SAFETY: The timer supplies a live task dialog and TDM_CLICK_BUTTON takes a scalar button ID.
        unsafe { SendMessageW(window, TDM_CLICK_BUTTON.cast_unsigned(), later, 0) };
    }
    S_OK
}
