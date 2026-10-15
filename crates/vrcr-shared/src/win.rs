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

#[cfg(windows)]
#[allow(unsafe_code)]
mod ctl {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    };

    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    fn open(pid: u32, access: u32) -> Option<Owned> {
        let handle = unsafe { OpenProcess(access, 0, pid) };
        (!handle.is_null()).then_some(Owned(handle))
    }

    pub fn process_image_path(pid: u32) -> Option<PathBuf> {
        let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut buf = vec![0u16; 32_768];
        let mut len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let ok =
            unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
        if ok == 0 {
            return None;
        }
        buf.truncate(usize::try_from(len).unwrap_or(0));
        Some(PathBuf::from(String::from_utf16_lossy(&buf)))
    }

    pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
        let Some(process) = open(pid, PROCESS_SYNCHRONIZE) else {
            return true;
        };
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        unsafe { WaitForSingleObject(process.0, ms) == WAIT_OBJECT_0 }
    }

    pub fn terminate_process(pid: u32) -> bool {
        let Some(process) = open(pid, PROCESS_TERMINATE) else {
            return false;
        };
        unsafe { TerminateProcess(process.0, 1) != 0 }
    }

    pub fn send_console_ctrl_c(pid: u32) -> bool {
        unsafe {
            FreeConsole();
            if AttachConsole(pid) == 0 {
                return false;
            }
            SetConsoleCtrlHandler(None, 1);
            let sent = GenerateConsoleCtrlEvent(0, 0) != 0;
            FreeConsole();
            sent
        }
    }

    pub fn ignore_ctrl_c(ignore: bool) -> bool {
        unsafe { SetConsoleCtrlHandler(None, i32::from(ignore)) != 0 }
    }

    pub fn free_disk_bytes(path: &Path) -> Option<u64> {
        let wide: Vec<u16> = OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect();
        let mut free = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
}

#[cfg(windows)]
pub use ctl::{
    free_disk_bytes, ignore_ctrl_c, process_image_path, send_console_ctrl_c, terminate_process, wait_for_exit,
};

#[cfg(not(windows))]
pub fn ignore_ctrl_c(_ignore: bool) -> bool {
    false
}

#[cfg(not(windows))]
pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(not(windows))]
pub fn wait_for_exit(pid: u32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while process_alive(pid) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    true
}

#[cfg(not(windows))]
pub fn terminate_process(_pid: u32) -> bool {
    false
}

#[cfg(not(windows))]
pub fn send_console_ctrl_c(_pid: u32) -> bool {
    false
}

#[cfg(not(windows))]
pub fn free_disk_bytes(_path: &std::path::Path) -> Option<u64> {
    None
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u16,
    pub ch: u16,
    pub ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlEvent {
    CtrlC,
    Close,
    Logoff,
    Shutdown,
}

pub fn ctrl_event(code: u32) -> Option<CtrlEvent> {
    match code {
        0 | 1 => Some(CtrlEvent::CtrlC),
        2 => Some(CtrlEvent::Close),
        5 => Some(CtrlEvent::Logoff),
        6 => Some(CtrlEvent::Shutdown),
        _ => None,
    }
}

pub const QUICK_EDIT_MODE: u32 = 0x0040;
pub const EXTENDED_FLAGS: u32 = 0x0080;

pub fn without_quick_edit(mode: u32) -> u32 {
    (mode & !QUICK_EDIT_MODE) | EXTENDED_FLAGS
}

pub type CtrlHandler = Box<dyn Fn(CtrlEvent) + Send + Sync>;

static CTRL_HANDLER: std::sync::OnceLock<CtrlHandler> = std::sync::OnceLock::new();

#[cfg(windows)]
#[allow(unsafe_code)]
mod console {
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
        RIGHT_CTRL_PRESSED, ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
        SetConsoleMode,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    use super::{CTRL_HANDLER, CtrlHandler, KeyEvent, QUICK_EDIT_MODE, ctrl_event, without_quick_edit};

    fn std_handle(which: u32) -> Option<HANDLE> {
        let h = unsafe { GetStdHandle(which) };
        (!h.is_null() && h != INVALID_HANDLE_VALUE).then_some(h)
    }

    fn mode_of(h: HANDLE) -> Option<u32> {
        let mut mode = 0u32;
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
        if unsafe { SetConsoleMode(h, without_quick_edit(mode)) } != 0 {
            "disabled"
        } else {
            "failed"
        }
    }

    pub fn console_width() -> Option<usize> {
        let h = std_handle(STD_OUTPUT_HANDLE)?;
        let mut info = unsafe { std::mem::zeroed::<CONSOLE_SCREEN_BUFFER_INFO>() };
        if unsafe { GetConsoleScreenBufferInfo(h, &mut info) } == 0 {
            return None;
        }
        let width = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
        usize::try_from(width).ok().filter(|w| *w > 0)
    }

    pub struct KeyReader {
        handle: HANDLE,
    }

    unsafe impl Send for KeyReader {}

    impl KeyReader {
        pub fn open() -> Option<KeyReader> {
            let handle = std_handle(STD_INPUT_HANDLE)?;
            mode_of(handle)?;
            Some(KeyReader { handle })
        }

        pub fn read(&self, timeout: Duration) -> io::Result<Option<KeyEvent>> {
            let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
            match unsafe { WaitForSingleObject(self.handle, ms) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => return Ok(None),
                _ => return Err(io::Error::last_os_error()),
            }
            let mut record = INPUT_RECORD::default();
            let mut read = 0u32;
            if unsafe { ReadConsoleInputW(self.handle, &mut record, 1, &mut read) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if read == 0 || u32::from(record.EventType) != KEY_EVENT {
                return Ok(None);
            }
            let key = unsafe { record.Event.KeyEvent };
            if key.bKeyDown == 0 {
                return Ok(None);
            }
            Ok(Some(KeyEvent {
                vk: key.wVirtualKeyCode,
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
            let wide: Vec<u16> = OsStr::new(name).encode_wide().chain(std::iter::once(0)).collect();
            let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
            if handle.is_null() {
                let e = io::Error::last_os_error();
                return Err(if e.raw_os_error() == i32::try_from(ERROR_ACCESS_DENIED).ok() {
                    MutexError::AccessDenied
                } else {
                    MutexError::Other(e)
                });
            }
            Ok(NamedMutex { handle, owned: false })
        }

        pub fn try_acquire(&mut self) -> bool {
            if !self.owned {
                let r = unsafe { WaitForSingleObject(self.handle, 0) };
                self.owned = r == WAIT_OBJECT_0 || r == WAIT_ABANDONED;
            }
            self.owned
        }
    }

    impl Drop for NamedMutex {
        fn drop(&mut self) {
            if self.owned {
                unsafe { ReleaseMutex(self.handle) };
            }
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(windows)]
pub use console::{
    KeyReader, MutexError, NamedMutex, console_width, disable_quick_edit, enable_vt_output,
    install_ctrl_handler,
};

#[cfg(not(windows))]
pub fn enable_vt_output() -> bool {
    true
}

#[cfg(not(windows))]
pub fn disable_quick_edit() -> &'static str {
    "unavailable"
}

#[cfg(not(windows))]
pub fn console_width() -> Option<usize> {
    None
}

#[cfg(not(windows))]
pub fn install_ctrl_handler(handler: CtrlHandler) -> bool {
    let _ = CTRL_HANDLER.set(handler);
    false
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod version {
    use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};

    pub fn os_description() -> String {
        let mut info = unsafe { std::mem::zeroed::<OSVERSIONINFOW>() };
        info.dwOSVersionInfoSize = u32::try_from(std::mem::size_of::<OSVERSIONINFOW>()).unwrap_or(0);
        if unsafe { GetVersionExW(&mut info) } == 0 {
            return "Microsoft Windows".to_owned();
        }
        format!(
            "Microsoft Windows {}.{}.{}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        )
    }
}

#[cfg(windows)]
pub use version::os_description;

#[cfg(not(windows))]
pub fn os_description() -> String {
    std::env::consts::OS.to_owned()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessUsage {
    pub cpu_100ns: u64,
    pub working_set: u64,
    pub peak_working_set: u64,
    pub private_bytes: u64,
    pub handles: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryStatus {
    pub total: u64,
    pub available: u64,
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod usage {
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount, GetProcessTimes};

    use super::{MemoryStatus, ProcessUsage};

    fn ticks(t: &FILETIME) -> u64 {
        (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn process_usage() -> Option<ProcessUsage> {
        let process = unsafe { GetCurrentProcess() };
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        if unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
            return None;
        }
        let mut mem = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
        let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(0);
        mem.cb = size;
        let mem_ok = unsafe { K32GetProcessMemoryInfo(process, (&raw mut mem).cast(), size) } != 0;
        let mut handles = 0u32;
        let handles_ok = unsafe { GetProcessHandleCount(process, &mut handles) } != 0;
        let bytes = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
        Some(ProcessUsage {
            cpu_100ns: ticks(&kernel).saturating_add(ticks(&user)),
            working_set: if mem_ok { bytes(mem.WorkingSetSize) } else { 0 },
            peak_working_set: if mem_ok { bytes(mem.PeakWorkingSetSize) } else { 0 },
            private_bytes: if mem_ok { bytes(mem.PrivateUsage) } else { 0 },
            handles: if handles_ok { handles } else { 0 },
        })
    }

    pub fn memory_status() -> Option<MemoryStatus> {
        let mut status = unsafe { std::mem::zeroed::<MEMORYSTATUSEX>() };
        status.dwLength = u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).unwrap_or(0);
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return None;
        }
        Some(MemoryStatus {
            total: status.ullTotalPhys,
            available: status.ullAvailPhys,
        })
    }

    pub fn cpu_name() -> Option<String> {
        let key = wide(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
        let value = wide("ProcessorNameString");
        let mut buf = [0u16; 256];
        let mut len = u32::try_from(std::mem::size_of_val(&buf)).unwrap_or(0);
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let chars = (len as usize / 2).min(buf.len());
        let name = String::from_utf16_lossy(&buf[..chars]);
        let name = name
            .trim_end_matches('\0')
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(windows)]
pub use usage::{cpu_name, memory_status, process_usage};

#[cfg(not(windows))]
pub fn process_usage() -> Option<ProcessUsage> {
    None
}

#[cfg(not(windows))]
pub fn memory_status() -> Option<MemoryStatus> {
    None
}

#[cfg(not(windows))]
pub fn cpu_name() -> Option<String> {
    None
}
