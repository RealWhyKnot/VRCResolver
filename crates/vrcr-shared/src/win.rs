#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_SUCCESS, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, RegCloseKey,
        RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub fn local_hms() -> (u16, u16, u16) {
        let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
        (t.wHour, t.wMinute, t.wSecond)
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn process_alive(pid: u32) -> bool {
        if pid == 0 {
            return true;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
        }
        let mut code = 0u32;
        let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
        unsafe { CloseHandle(handle) };
        ok == 0 || code == STILL_ACTIVE as u32
    }

    pub struct HkcuKey {
        key: HKEY,
    }

    unsafe impl Send for HkcuKey {}

    impl HkcuKey {
        pub fn create(path: &str) -> io::Result<HkcuKey> {
            let sub = wide(path);
            let mut key: HKEY = std::ptr::null_mut();
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
            let access = if writable { KEY_READ | KEY_WRITE } else { KEY_READ };
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
            let rc = unsafe { RegSetValueExW(self.key, name.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4) };
            check(rc)
        }

        pub fn delete_value(&self, name: &str) -> io::Result<()> {
            let name = wide(name);
            let rc = unsafe { RegDeleteValueW(self.key, name.as_ptr()) };
            if rc == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            check(rc)
        }
    }

    impl Drop for HkcuKey {
        fn drop(&mut self) {
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
}

#[cfg(windows)]
pub use imp::{HkcuKey, local_hms, process_alive};

#[cfg(not(windows))]
pub fn local_hms() -> (u16, u16, u16) {
    let s = crate::logutil::UtcStamp::at(std::time::SystemTime::now());
    let part = |v: u64| u16::try_from(v).unwrap_or(0);
    (part(s.hour), part(s.minute), part(s.second))
}

#[cfg(windows)]
pub struct RegDword<'a> {
    pub key: &'a HkcuKey,
    pub name: &'a str,
}

#[cfg(windows)]
impl crate::wininet::DwordStore for RegDword<'_> {
    fn get(&self) -> Option<u32> {
        self.key.get_dword(self.name)
    }

    fn set(&mut self, value: u32) -> std::io::Result<()> {
        self.key.set_dword(self.name, value)
    }

    fn delete(&mut self) -> std::io::Result<()> {
        self.key.delete_value(self.name)
    }
}

#[cfg(not(windows))]
pub fn process_alive(pid: u32) -> bool {
    pid == 0 || std::path::Path::new(&format!("/proc/{pid}")).exists()
}
