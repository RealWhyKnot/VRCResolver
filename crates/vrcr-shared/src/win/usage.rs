use windows_sys::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessHandleCount, GetProcessTimes,
};

use super::{MemoryStatus, ProcessUsage};

fn ticks(t: &FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn process_usage() -> Option<ProcessUsage> {
    // SAFETY: GetCurrentProcess takes no pointers and returns the current-process pseudo-handle.
    let process = unsafe { GetCurrentProcess() };
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: process is the current-process handle and each FILETIME output is a distinct writable value.
    if unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return None;
    }
    // SAFETY: PROCESS_MEMORY_COUNTERS_EX contains only integer fields, valid when zeroed.
    let mut mem = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(0);
    mem.cb = size;
    // SAFETY: mem is writable for size bytes, starts with the required prefix, and cb equals size.
    let mem_ok = unsafe { K32GetProcessMemoryInfo(process, (&raw mut mem).cast(), size) } != 0;
    let mut handles = 0u32;
    // SAFETY: process is the current-process handle and handles is a live writable u32.
    let handles_ok = unsafe { GetProcessHandleCount(process, &mut handles) } != 0;
    let bytes = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
    Some(ProcessUsage {
        cpu_100ns: ticks(&kernel).saturating_add(ticks(&user)),
        working_set: if mem_ok { bytes(mem.WorkingSetSize) } else { 0 },
        peak_working_set: if mem_ok {
            bytes(mem.PeakWorkingSetSize)
        } else {
            0
        },
        private_bytes: if mem_ok { bytes(mem.PrivateUsage) } else { 0 },
        handles: if handles_ok { handles } else { 0 },
    })
}

pub fn memory_status() -> Option<MemoryStatus> {
    // SAFETY: MEMORYSTATUSEX contains only integer fields, valid when zeroed.
    let mut status = unsafe { std::mem::zeroed::<MEMORYSTATUSEX>() };
    status.dwLength = u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).unwrap_or(0);
    // SAFETY: status is writable with dwLength initialized to its actual size.
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
    // SAFETY: key and value are NUL-terminated and len is the byte capacity of the writable UTF-16 buffer.
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
    let chars = (usize::try_from(len).expect("Windows pointer width holds the buffer length") / 2)
        .min(buf.len());
    let name = String::from_utf16_lossy(&buf[..chars]);
    let name = name
        .trim_end_matches('\0')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!name.is_empty()).then_some(name)
}
