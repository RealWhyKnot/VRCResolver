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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineStore {
    Root,
    My,
}

impl MachineStore {
    pub fn name(self) -> &'static str {
        match self {
            MachineStore::Root => "ROOT",
            MachineStore::My => "MY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elevated {
    Exited(u32),
    Declined,
    TimedOut,
    Failed(String),
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod crypt {
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_CANCELLED, GetLastError, HLOCAL, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CERT_CONTEXT, CERT_STORE_ADD_USE_EXISTING, CERT_STORE_OPEN_EXISTING_FLAG, CERT_STORE_PROV_SYSTEM_W,
        CERT_STORE_READONLY_FLAG, CERT_SYSTEM_STORE_LOCAL_MACHINE, CRYPT_INTEGER_BLOB,
        CRYPTPROTECT_UI_FORBIDDEN, CertAddEncodedCertificateToStore, CertCloseStore,
        CertDeleteCertificateFromStore, CertDuplicateCertificateContext, CertEnumCertificatesInStore,
        CertOpenStore, CryptProtectData, CryptUnprotectData, HCERTSTORE, PKCS_7_ASN_ENCODING,
        X509_ASN_ENCODING,
    };
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    use super::{Elevated, MachineStore};

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    struct Store(HCERTSTORE);

    impl Store {
        fn open(which: MachineStore, writable: bool) -> io::Result<Store> {
            let name = wide(OsStr::new(which.name()));
            let mut flags = CERT_SYSTEM_STORE_LOCAL_MACHINE | CERT_STORE_OPEN_EXISTING_FLAG;
            if !writable {
                flags |= CERT_STORE_READONLY_FLAG;
            }
            let handle =
                unsafe { CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, flags, name.as_ptr().cast()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Store(handle))
        }

        fn each(&self, mut visit: impl FnMut(*const CERT_CONTEXT, &[u8])) {
            let mut ctx: *const CERT_CONTEXT = std::ptr::null();
            loop {
                ctx = unsafe { CertEnumCertificatesInStore(self.0, ctx) };
                if ctx.is_null() {
                    return;
                }
                let der = unsafe {
                    let c = &*ctx;
                    std::slice::from_raw_parts(c.pbCertEncoded, c.cbCertEncoded as usize)
                };
                visit(ctx, der);
            }
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            unsafe { CertCloseStore(self.0, 0) };
        }
    }

    pub fn machine_store_certs(which: MachineStore) -> io::Result<Vec<Vec<u8>>> {
        let store = Store::open(which, false)?;
        let mut out = Vec::new();
        store.each(|_, der| out.push(der.to_vec()));
        Ok(out)
    }

    pub fn machine_store_add(which: MachineStore, der: &[u8]) -> io::Result<()> {
        let store = Store::open(which, true)?;
        let len = u32::try_from(der.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let ok = unsafe {
            CertAddEncodedCertificateToStore(
                store.0,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                der.as_ptr(),
                len,
                CERT_STORE_ADD_USE_EXISTING,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn machine_store_remove(which: MachineStore, remove: impl Fn(&[u8]) -> bool) -> io::Result<usize> {
        let store = Store::open(which, true)?;
        let mut removed = 0;
        let mut failure: Option<io::Error> = None;
        store.each(|ctx, der| {
            if !remove(der) {
                return;
            }
            let dup = unsafe { CertDuplicateCertificateContext(ctx) };
            if unsafe { CertDeleteCertificateFromStore(dup) } == 0 {
                failure.get_or_insert_with(io::Error::last_os_error);
            } else {
                removed += 1;
            }
        });
        match failure {
            Some(e) => Err(e),
            None => Ok(removed),
        }
    }

    fn blob(data: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?,
            pbData: data.as_ptr().cast_mut(),
        })
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as HLOCAL) };
        bytes
    }

    pub fn dpapi_protect(data: &[u8]) -> io::Result<Vec<u8>> {
        let input = blob(data)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(take(out))
    }

    pub fn dpapi_unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
        let input = blob(data)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(take(out))
    }

    pub fn run_elevated(exe: &Path, args: &str, timeout: Duration) -> Elevated {
        let verb = wide(OsStr::new("runas"));
        let file = wide(exe.as_os_str());
        let params = wide(OsStr::new(args));
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(u32::MAX);
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0;
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let code = unsafe { GetLastError() };
            if code == ERROR_CANCELLED {
                return Elevated::Declined;
            }
            return Elevated::Failed(
                io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX)).to_string(),
            );
        }
        if info.hProcess.is_null() {
            return Elevated::Exited(0);
        }
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let waited = unsafe { WaitForSingleObject(info.hProcess, ms) };
        let result = if waited == WAIT_OBJECT_0 {
            let mut code = 0u32;
            if unsafe { GetExitCodeProcess(info.hProcess, &mut code) } == 0 {
                Elevated::Failed(io::Error::last_os_error().to_string())
            } else {
                Elevated::Exited(code)
            }
        } else if waited == WAIT_TIMEOUT {
            Elevated::TimedOut
        } else {
            Elevated::Failed(io::Error::last_os_error().to_string())
        };
        unsafe { CloseHandle(info.hProcess) };
        result
    }
}

#[cfg(windows)]
pub use crypt::{
    dpapi_protect, dpapi_unprotect, machine_store_add, machine_store_certs, machine_store_remove,
    run_elevated,
};

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
