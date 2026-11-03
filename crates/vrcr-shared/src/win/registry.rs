use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_SUCCESS, GetLastError,
    STILL_ACTIVE,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub fn local_hms() -> (u16, u16, u16) {
    // SAFETY: SYSTEMTIME contains only integers, so an all-zero value is valid.
    let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    // SAFETY: t is a live, writable SYSTEMTIME for the duration of the call.
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    (t.wHour, t.wMinute, t.wSecond)
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    // SAFETY: OpenProcess takes scalar arguments and returns an owned handle or null.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // SAFETY: GetLastError reads thread-local error state and takes no pointers.
        return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
    }
    let mut code = 0u32;
    // SAFETY: handle is open and code is a live u32 output buffer.
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    // SAFETY: handle is the non-null handle opened above and is closed exactly once.
    unsafe { CloseHandle(handle) };
    ok == 0 || code == STILL_ACTIVE.cast_unsigned()
}

pub struct HkcuKey {
    key: HKEY,
}

// SAFETY: Registry handles can cross threads; this type uniquely owns and closes its handle.
unsafe impl Send for HkcuKey {}

impl HkcuKey {
    pub fn create(path: &str) -> io::Result<HkcuKey> {
        let sub = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: sub is NUL-terminated and key is writable; the predefined root handle is valid.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        check(rc).map(|()| HkcuKey { key })
    }

    pub fn open(path: &str, writable: bool) -> io::Result<Option<HkcuKey>> {
        let sub = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        let access = if writable {
            KEY_READ | KEY_WRITE
        } else {
            KEY_READ
        };
        // SAFETY: sub is NUL-terminated and key is writable; the predefined root handle is valid.
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, access, &mut key) };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(rc).map(|()| Some(HkcuKey { key }))
    }

    pub fn get_dword(&self, name: &str) -> Option<u32> {
        let name = wide(name);
        let mut kind = 0u32;
        let mut value = 0u32;
        let mut len = 4u32;
        // SAFETY: self owns the open key, name is NUL-terminated, and len bounds the four-byte output.
        let rc = unsafe {
            RegQueryValueExW(
                self.key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                (&raw mut value).cast(),
                &mut len,
            )
        };
        (rc == ERROR_SUCCESS && kind == REG_DWORD && len == 4).then_some(value)
    }

    pub fn set_dword(&self, name: &str, value: u32) -> io::Result<()> {
        let name = wide(name);
        let bytes = value.to_le_bytes();
        // SAFETY: self owns the key, name is NUL-terminated, and bytes holds the four input bytes.
        let rc =
            unsafe { RegSetValueExW(self.key, name.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4) };
        check(rc)
    }

    pub fn delete_value(&self, name: &str) -> io::Result<()> {
        let name = wide(name);
        // SAFETY: self owns the open key and name remains NUL-terminated throughout the call.
        let rc = unsafe { RegDeleteValueW(self.key, name.as_ptr()) };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(rc)
    }
}

impl Drop for HkcuKey {
    fn drop(&mut self) {
        // SAFETY: self uniquely owns the successfully opened registry handle.
        unsafe { RegCloseKey(self.key) };
    }
}

fn check(rc: u32) -> io::Result<()> {
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(
            i32::try_from(rc).unwrap_or(i32::MAX),
        ))
    }
}
