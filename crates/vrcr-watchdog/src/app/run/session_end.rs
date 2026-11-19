use std::cell::Cell;
use std::io;
use std::ptr::{null, null_mut};
use std::sync::Arc;

use vrcr_shared::console::{Component, Ux};
use vrcr_shared::win::CtrlEvent;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, ENDSESSION_LOGOFF,
    GetMessageW, MSG, PostQuitMessage, RegisterClassW, SetProcessDPIAware, UnregisterClassW,
    WM_DESTROY, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_POPUP,
};

use super::Lifecycle;

thread_local! {
    static SESSION_LIFE: Cell<Option<Arc<Lifecycle>>> = const { Cell::new(None) };
}

pub(super) fn listen(life: &Arc<Lifecycle>, ux: &Ux) {
    // SAFETY: This sets the process's system DPI awareness before creating either native window.
    unsafe { SetProcessDPIAware() };
    let life = Arc::clone(life);
    let window_ux = ux.clone();
    let started = std::thread::Builder::new()
        .name("windows-session-end".to_owned())
        .spawn(move || {
            if let Err(error) = run(life) {
                window_ux.log_warn(
                    Component::Shutdown,
                    &format!("Could not listen for Windows sign-out and shutdown: {error}"),
                );
            }
        });
    if let Err(error) = started {
        ux.log_warn(
            Component::Shutdown,
            &format!("Could not start the Windows shutdown listener: {error}"),
        );
    }
}

fn run(life: Arc<Lifecycle>) -> io::Result<()> {
    let name: Vec<u16> = "VRCResolverSessionEnd"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: A null name requests the handle of the current executable without taking ownership.
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error());
    }
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: name.as_ptr(),
        ..WNDCLASSW::default()
    };
    // SAFETY: The initialized class uses a valid module, static window procedure and NUL-terminated name.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    SESSION_LIFE.set(Some(life));
    // SAFETY: The class is registered in this module and the remaining pointers are optional and null.
    let window = unsafe {
        CreateWindowExW(
            0,
            name.as_ptr(),
            null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    let result = if window.is_null() {
        Err(io::Error::last_os_error())
    } else {
        let result = pump_messages();
        // SAFETY: This thread created the window and is responsible for destroying it on exit.
        unsafe { DestroyWindow(window) };
        result
    };
    // SAFETY: The class is registered by this thread, with no windows left, and name is still NUL-terminated.
    unsafe { UnregisterClassW(name.as_ptr(), instance) };
    result
}

fn pump_messages() -> io::Result<()> {
    let mut message = MSG::default();
    loop {
        // SAFETY: The message buffer is writable and a null window reads this thread's entire queue.
        match unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } {
            -1 => return Err(io::Error::last_os_error()),
            0 => return Ok(()),
            _ => {
                // SAFETY: GetMessageW succeeded and initialized the message for this thread to dispatch.
                unsafe { DispatchMessageW(&message) };
            }
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0
                && let Some(life) = SESSION_LIFE.take()
            {
                let event = if lparam.cast_unsigned()
                    & usize::try_from(ENDSESSION_LOGOFF).expect("logoff flag fits LPARAM")
                    != 0
                {
                    CtrlEvent::Logoff
                } else {
                    CtrlEvent::Shutdown
                };
                life.on_ctrl(event);
                // SAFETY: This callback runs on the listener thread and posts quit to its own queue.
                unsafe { PostQuitMessage(0) };
            }
            0
        }
        WM_DESTROY => {
            // SAFETY: The window is being destroyed on its listener thread, whose queue can now exit.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: Forward unhandled messages with their original parameters to the Windows default procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
