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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultOutput {
    NoDevices,
    NoPreferred,
    Unknown,
    Opened { result: u32, device: Option<String> },
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod audio {
    use windows_sys::Win32::Media::Audio::{
        HWAVEOUT, WAVEFORMATEX, WAVEOUTCAPSW, waveOutClose, waveOutGetDevCapsW, waveOutGetNumDevs,
        waveOutMessage, waveOutOpen,
    };
    use windows_sys::Win32::Media::Multimedia::DRVM_MAPPER_PREFERRED_GET;

    use super::DefaultOutput;

    const NO_PREFERRED_DEVICE: u32 = u32::MAX;

    pub fn probe_default_output() -> DefaultOutput {
        if unsafe { waveOutGetNumDevs() } == 0 {
            return DefaultOutput::NoDevices;
        }
        let mut device_id: u32 = 0;
        let mut status: u32 = 0;
        let mapper: HWAVEOUT = std::ptr::without_provenance_mut(usize::MAX);
        let r = unsafe {
            waveOutMessage(
                mapper,
                DRVM_MAPPER_PREFERRED_GET,
                std::ptr::from_mut(&mut device_id) as usize,
                std::ptr::from_mut(&mut status) as usize,
            )
        };
        if r != 0 {
            return DefaultOutput::Unknown;
        }
        if device_id == NO_PREFERRED_DEVICE {
            return DefaultOutput::NoPreferred;
        }
        let format = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 192_000,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut handle: HWAVEOUT = std::ptr::null_mut();
        let result = unsafe { waveOutOpen(&mut handle, device_id, &format, 0, 0, 0) };
        if result == 0 {
            unsafe { waveOutClose(handle) };
        }
        DefaultOutput::Opened {
            result,
            device: device_name(device_id),
        }
    }

    fn device_name(device_id: u32) -> Option<String> {
        let mut caps = WAVEOUTCAPSW::default();
        let size = u32::try_from(size_of::<WAVEOUTCAPSW>()).unwrap_or(0);
        if unsafe { waveOutGetDevCapsW(device_id as usize, &mut caps, size) } != 0 {
            return None;
        }
        let raw = caps.szPname;
        let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        let name = String::from_utf16_lossy(&raw[..len]);
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(windows)]
pub use audio::probe_default_output;

#[cfg(not(windows))]
pub fn probe_default_output() -> DefaultOutput {
    DefaultOutput::Unknown
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub exe: String,
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod procs {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::ProcessInfo;

    const FILETIME_TO_UNIX_SECS: u64 = 11_644_473_600;

    pub fn list_processes() -> Vec<ProcessInfo> {
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
        entry.dwSize = u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0);
        let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
        while ok != 0 {
            let raw = entry.szExeFile;
            let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
            out.push(ProcessInfo {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                exe: String::from_utf16_lossy(&raw[..len]),
            });
            ok = unsafe { Process32NextW(snap, &mut entry) };
        }
        unsafe { CloseHandle(snap) };
        out
    }

    pub fn process_start_time(pid: u32) -> Option<SystemTime> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return None;
        }
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        let since_1601 = Duration::from_nanos(ticks.saturating_mul(100));
        since_1601
            .checked_sub(Duration::from_secs(FILETIME_TO_UNIX_SECS))
            .map(|d| UNIX_EPOCH + d)
    }
}

#[cfg(windows)]
pub use procs::{list_processes, process_start_time};

#[cfg(not(windows))]
pub fn list_processes() -> Vec<ProcessInfo> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn process_start_time(_pid: u32) -> Option<std::time::SystemTime> {
    None
}

pub fn process_stem(exe: &str) -> &str {
    let len = exe.len();
    if len > 4 && exe.is_char_boundary(len - 4) && exe[len - 4..].eq_ignore_ascii_case(".exe") {
        &exe[..len - 4]
    } else {
        exe
    }
}

pub fn describe_parent(processes: &[ProcessInfo], own_pid: u32) -> Option<String> {
    let me = processes.iter().find(|p| p.pid == own_pid)?;
    let parent = processes
        .iter()
        .find(|p| p.pid == me.parent_pid && p.pid != own_pid)?;
    Some(format!("{} pid={}", process_stem(&parent.exe), parent.pid))
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod mf {
    use std::ffi::c_void;

    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::core::{GUID, HRESULT};

    #[repr(C)]
    struct RegisterTypeInfo {
        major: GUID,
        subtype: GUID,
    }

    #[link(name = "mfplat", kind = "raw-dylib")]
    unsafe extern "system" {
        fn MFStartup(version: u32, flags: u32) -> HRESULT;
        fn MFShutdown() -> HRESULT;
        fn MFTEnumEx(
            category: GUID,
            flags: u32,
            input: *const RegisterTypeInfo,
            output: *const RegisterTypeInfo,
            activates: *mut *mut *mut c_void,
            count: *mut u32,
        ) -> HRESULT;
    }

    const MF_VERSION: u32 = 0x0002_0070;
    const MFSTARTUP_LITE: u32 = 1;
    const MFT_ENUM_SYNC_ASYNC_HARDWARE: u32 = 0x1 | 0x2 | 0x4;
    const CATEGORY_VIDEO_DECODER: GUID = GUID::from_u128(0xd6c02d4b_6833_45b4_971a_05a4b04bab91);
    const MEDIA_TYPE_VIDEO: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00aa00389b71);

    type Release = unsafe extern "system" fn(*mut c_void) -> u32;

    pub struct MediaFoundation(());

    impl MediaFoundation {
        pub fn start() -> Option<MediaFoundation> {
            (unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) } >= 0).then_some(MediaFoundation(()))
        }

        pub fn has_video_decoder(&self, fourcc: u32) -> Option<bool> {
            let input = RegisterTypeInfo {
                major: MEDIA_TYPE_VIDEO,
                subtype: GUID {
                    data1: fourcc,
                    data2: 0x0000,
                    data3: 0x0010,
                    data4: [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
                },
            };
            let mut activates: *mut *mut c_void = std::ptr::null_mut();
            let mut count: u32 = 0;
            let hr = unsafe {
                MFTEnumEx(
                    CATEGORY_VIDEO_DECODER,
                    MFT_ENUM_SYNC_ASYNC_HARDWARE,
                    &input,
                    std::ptr::null(),
                    &mut activates,
                    &mut count,
                )
            };
            if hr < 0 {
                return None;
            }
            if !activates.is_null() {
                for i in 0..count as usize {
                    let p = unsafe { *activates.add(i) };
                    if !p.is_null() {
                        let vtbl = unsafe { *p.cast::<*const Release>() };
                        let release = unsafe { *vtbl.add(2) };
                        unsafe { release(p) };
                    }
                }
                unsafe { CoTaskMemFree(activates.cast()) };
            }
            Some(count > 0)
        }
    }

    impl Drop for MediaFoundation {
        fn drop(&mut self) {
            unsafe { MFShutdown() };
        }
    }
}

#[cfg(windows)]
pub use mf::MediaFoundation;

#[cfg(not(windows))]
pub struct MediaFoundation(());

#[cfg(not(windows))]
impl MediaFoundation {
    pub fn start() -> Option<MediaFoundation> {
        None
    }

    pub fn has_video_decoder(&self, _fourcc: u32) -> Option<bool> {
        None
    }
}

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
