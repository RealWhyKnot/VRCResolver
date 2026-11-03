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
        // SAFETY: Owned is constructed only from a non-null owned process handle and closes it once.
        unsafe { CloseHandle(self.0) };
    }
}

fn open(pid: u32, access: u32) -> Option<Owned> {
    // SAFETY: OpenProcess takes scalar arguments and returns an owned handle or null.
    let handle = unsafe { OpenProcess(access, 0, pid) };
    (!handle.is_null()).then_some(Owned(handle))
}

pub fn process_image_path(pid: u32) -> Option<PathBuf> {
    let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut buf = vec![0u16; 32_768];
    let mut len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    let ok =
        // SAFETY: process owns an open handle and len is the writable UTF-16 buffer capacity.
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
    // SAFETY: process owns an open handle with synchronization access for this wait.
    unsafe { WaitForSingleObject(process.0, ms) == WAIT_OBJECT_0 }
}

pub fn terminate_process(pid: u32) -> bool {
    let Some(process) = open(pid, PROCESS_TERMINATE) else {
        return false;
    };
    // SAFETY: process owns an open handle with termination access; the exit code is a scalar.
    unsafe { TerminateProcess(process.0, 1) != 0 }
}

pub fn send_console_ctrl_c(pid: u32) -> bool {
    // SAFETY: Console APIs use scalar process IDs or a null callback and retain no borrowed pointers.
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
    // SAFETY: The null callback selects the process ignore flag and no borrowed pointers are retained.
    unsafe { SetConsoleCtrlHandler(None, i32::from(ignore)) != 0 }
}

pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free = 0u64;
    // SAFETY: wide is NUL-terminated and free is a live writable u64; unused outputs are null.
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
