use std::cell::Cell;
use std::io;
use std::ptr::{self, null_mut};

use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Component, Ux};
use windows_sys::Win32::Foundation::{HWND, LPARAM, S_OK, WPARAM};
use windows_sys::Win32::UI::Controls::{
    TASKDIALOGCONFIG, TDCBF_NO_BUTTON, TDCBF_YES_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_CALLBACK_TIMER, TDM_CLICK_BUTTON, TDN_CREATED, TDN_TIMER, TaskDialogIndirect,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, IDNO, IDYES, SWP_NOMOVE, SWP_NOSIZE, SendMessageW, SetWindowPos,
};
use windows_sys::core::HRESULT;

use super::setup::{Changes, Decision};

const TIMEOUT_MS: u32 = 60_000;
const TITLE: &str = "VRCResolver needs your permission";

struct DialogState<'a> {
    closing: &'a CancellationToken,
    ux: &'a Ux,
    stopped: Cell<Option<Decision>>,
}

pub(super) fn ask(changes: Changes, closing: &CancellationToken, ux: &Ux) -> Decision {
    if closing.is_cancelled() {
        return Decision::Closed;
    }
    let change = match changes {
        Changes::Hosts => "Windows will ask to add one line to a system file (Windows hosts file).",
        Changes::Certificate => "Windows will ask to trust a security certificate on this PC.",
        Changes::Both => {
            "Windows will ask to add one line to a system file (Windows hosts file) and trust a security certificate on this PC."
        }
    };
    let content = format!(
        "{change}\n\nThis only affects videos in public VRChat worlds. It's safe to choose No."
    );
    let title: Vec<u16> = TITLE.encode_utf16().chain(Some(0)).collect();
    let content: Vec<u16> = content.encode_utf16().chain(Some(0)).collect();
    let state = DialogState {
        closing,
        ux,
        stopped: Cell::new(None),
    };
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(std::mem::size_of::<TASKDIALOGCONFIG>())
            .expect("dialog config size fits a u32"),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_CALLBACK_TIMER,
        dwCommonButtons: TDCBF_YES_BUTTON | TDCBF_NO_BUTTON,
        nDefaultButton: IDYES,
        pszWindowTitle: title.as_ptr(),
        pszContent: content.as_ptr(),
        pfCallback: Some(dialog_callback),
        lpCallbackData: ptr::from_ref(&state).expose_provenance().cast_signed(),
        ..TASKDIALOGCONFIG::default()
    };
    let mut button = 0;
    // SAFETY: The config, NUL-terminated strings, callback state and button output live until this modal call returns.
    let result = unsafe { TaskDialogIndirect(&config, &mut button, null_mut(), null_mut()) };
    if let Some(reason) = state.stopped.get() {
        return reason;
    }
    if closing.is_cancelled() {
        return Decision::Closed;
    }
    if result < 0 {
        let error = io::Error::from_raw_os_error(result);
        ux.log_warn(
            Component::Terminal,
            &format!("Could not open the Windows permission dialog: {error}"),
        );
        return Decision::Closed;
    }
    match button {
        IDYES => Decision::Yes,
        IDNO => Decision::No,
        _ => Decision::Closed,
    }
}

unsafe extern "system" fn dialog_callback(
    window: HWND,
    notification: u32,
    elapsed: WPARAM,
    _lparam: LPARAM,
    callback_data: isize,
) -> HRESULT {
    // SAFETY: TaskDialogIndirect passes back the exposed pointer to the live DialogState during the modal call.
    let state =
        unsafe { &*ptr::with_exposed_provenance::<DialogState<'_>>(callback_data.cast_unsigned()) };
    match notification.cast_signed() {
        TDN_CREATED => {
            // SAFETY: The creation notification supplies a live dialog window owned by the calling thread.
            let positioned =
                unsafe { SetWindowPos(window, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE) };
            if positioned == 0 {
                let error = io::Error::last_os_error();
                state.ux.log_warn(
                    Component::Terminal,
                    &format!("Could not put the Windows permission dialog on top: {error}"),
                );
            }
        }
        TDN_TIMER => {
            let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
            let reason = if state.closing.is_cancelled() {
                Decision::Closed
            } else if elapsed >= TIMEOUT_MS {
                Decision::TimedOut
            } else {
                return S_OK;
            };
            state.stopped.set(Some(reason));
            let no_button = WPARAM::try_from(IDNO).expect("No button ID fits WPARAM");
            // SAFETY: The timer supplies a live task dialog and TDM_CLICK_BUTTON takes a scalar button ID.
            unsafe { SendMessageW(window, TDM_CLICK_BUTTON.cast_unsigned(), no_button, 0) };
        }
        _ => {}
    }
    S_OK
}
