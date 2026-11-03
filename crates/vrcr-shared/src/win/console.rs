use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
    GetConsoleScreenBufferInfo, GetStdHandle, INPUT_RECORD, KEY_EVENT, LEFT_CTRL_PRESSED,
    RIGHT_CTRL_PRESSED, ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    SetConsoleCtrlHandler, SetConsoleMode,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

use super::{CTRL_HANDLER, CtrlHandler, KeyEvent, QUICK_EDIT_MODE, ctrl_event, without_quick_edit};

fn std_handle(which: u32) -> Option<HANDLE> {
    // SAFETY: GetStdHandle takes a scalar selector and returns a borrowed handle or failure sentinel.
    let h = unsafe { GetStdHandle(which) };
    (!h.is_null() && h != INVALID_HANDLE_VALUE).then_some(h)
}

fn mode_of(h: HANDLE) -> Option<u32> {
    let mut mode = 0u32;
    // SAFETY: h is a checked standard handle and mode is a live writable u32.
    (unsafe { GetConsoleMode(h, &mut mode) } != 0).then_some(mode)
}

pub fn enable_vt_output() -> bool {
    let Some(h) = std_handle(STD_OUTPUT_HANDLE) else {
        return false;
    };
    let Some(mode) = mode_of(h) else {
        return false;
    };
    if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 {
        return true;
    }
    // SAFETY: h is a checked console handle and mode only adds the documented output flag.
    unsafe { SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0 }
}

pub fn disable_quick_edit() -> &'static str {
    let Some(h) = std_handle(STD_INPUT_HANDLE) else {
        return "unavailable";
    };
    let Some(mode) = mode_of(h) else {
        return "unavailable";
    };
    if mode & QUICK_EDIT_MODE == 0 {
        return "already_off";
    }
    // SAFETY: h is a checked console handle and the mode preserves all unrelated flags.
    if unsafe { SetConsoleMode(h, without_quick_edit(mode)) } != 0 {
        "disabled"
    } else {
        "failed"
    }
}

pub fn console_width() -> Option<usize> {
    let h = std_handle(STD_OUTPUT_HANDLE)?;
    // SAFETY: CONSOLE_SCREEN_BUFFER_INFO contains only integer fields, valid when zeroed.
    let mut info = unsafe { std::mem::zeroed::<CONSOLE_SCREEN_BUFFER_INFO>() };
    // SAFETY: h is a checked standard handle and info is a live writable buffer of the required type.
    if unsafe { GetConsoleScreenBufferInfo(h, &mut info) } == 0 {
        return None;
    }
    let width = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
    usize::try_from(width).ok().filter(|w| *w > 0)
}

pub struct KeyReader {
    handle: HANDLE,
}

// SAFETY: Windows console handles may be used on another thread; this type never frees the borrowed handle.
unsafe impl Send for KeyReader {}

impl KeyReader {
    pub fn open() -> Option<KeyReader> {
        let handle = std_handle(STD_INPUT_HANDLE)?;
        mode_of(handle)?;
        Some(KeyReader { handle })
    }

    pub fn read(&self, timeout: Duration) -> io::Result<Option<KeyEvent>> {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: self.handle is the checked borrowed standard-input handle and remains owned by the process.
        match unsafe { WaitForSingleObject(self.handle, ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(None),
            _ => return Err(io::Error::last_os_error()),
        }
        let mut record = INPUT_RECORD::default();
        let mut read = 0u32;
        // SAFETY: record is writable for one INPUT_RECORD and read is a live writable count.
        if unsafe { ReadConsoleInputW(self.handle, &mut record, 1, &mut read) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 || u32::from(record.EventType) != KEY_EVENT {
            return Ok(None);
        }
        // SAFETY: read is nonzero and EventType was checked against KEY_EVENT before selecting the union field.
        let key = unsafe { record.Event.KeyEvent };
        if key.bKeyDown == 0 {
            return Ok(None);
        }
        Ok(Some(KeyEvent {
            vk: key.wVirtualKeyCode,
            // SAFETY: ReadConsoleInputW filled a KEY_EVENT_RECORD with the Unicode member of uChar.
            ch: unsafe { key.uChar.UnicodeChar },
            ctrl: key.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0,
        }))
    }
}

unsafe extern "system" fn on_ctrl(code: u32) -> windows_sys::core::BOOL {
    let (Some(event), Some(handler)) = (ctrl_event(code), CTRL_HANDLER.get()) else {
        return 0;
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(event)));
    1
}

pub fn install_ctrl_handler(handler: CtrlHandler) -> bool {
    if CTRL_HANDLER.set(handler).is_err() {
        return false;
    }
    // SAFETY: on_ctrl has the required system ABI, static lifetime, and catches unwinding before returning.
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) != 0 }
}

pub enum MutexError {
    AccessDenied,
    Other(io::Error),
}

pub struct NamedMutex {
    handle: HANDLE,
    owned: bool,
}

impl NamedMutex {
    pub fn create(name: &str) -> Result<NamedMutex, MutexError> {
        let wide: Vec<u16> = OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: wide is a live NUL-terminated name and null security attributes request the default descriptor.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        if handle.is_null() {
            let e = io::Error::last_os_error();
            return Err(
                if e.raw_os_error() == i32::try_from(ERROR_ACCESS_DENIED).ok() {
                    MutexError::AccessDenied
                } else {
                    MutexError::Other(e)
                },
            );
        }
        Ok(NamedMutex {
            handle,
            owned: false,
        })
    }

    pub fn try_acquire(&mut self) -> bool {
        if !self.owned {
            // SAFETY: self owns a non-null mutex handle returned by CreateMutexW.
            let r = unsafe { WaitForSingleObject(self.handle, 0) };
            self.owned = r == WAIT_OBJECT_0 || r == WAIT_ABANDONED;
        }
        self.owned
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: owned is set only after a successful wait acquired this mutex on the calling thread.
            unsafe { ReleaseMutex(self.handle) };
        }
        // SAFETY: self uniquely owns this mutex handle and closes it exactly once.
        unsafe { CloseHandle(self.handle) };
    }
}
