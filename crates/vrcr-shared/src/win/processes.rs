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
    // SAFETY: The snapshot API takes only scalar arguments and returns an owned handle or failure.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: PROCESSENTRY32W contains only integers and fixed-size arrays, valid when zeroed.
    let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
    entry.dwSize = u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0);
    // SAFETY: snap is a valid snapshot and entry is writable with dwSize initialized.
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
    while ok != 0 {
        let raw = entry.szExeFile;
        let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        out.push(ProcessInfo {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            exe: String::from_utf16_lossy(&raw[..len]),
        });
        // SAFETY: snap remains open and entry is the same initialized writable PROCESSENTRY32W.
        ok = unsafe { Process32NextW(snap, &mut entry) };
    }
    // SAFETY: snap is the successfully opened snapshot handle and is closed exactly once.
    unsafe { CloseHandle(snap) };
    out
}

pub fn process_start_time(pid: u32) -> Option<SystemTime> {
    // SAFETY: OpenProcess takes scalar arguments and returns an owned handle or null.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: handle is open and each FILETIME output points to a distinct live writable value.
    let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    // SAFETY: handle is the successfully opened process handle and is closed exactly once.
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
