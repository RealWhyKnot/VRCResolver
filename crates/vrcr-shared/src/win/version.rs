use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};

pub fn os_description() -> String {
    // SAFETY: OSVERSIONINFOW contains only integer fields and a fixed-size array, valid when zeroed.
    let mut info = unsafe { std::mem::zeroed::<OSVERSIONINFOW>() };
    info.dwOSVersionInfoSize = u32::try_from(std::mem::size_of::<OSVERSIONINFOW>()).unwrap_or(0);
    // SAFETY: info is writable with dwOSVersionInfoSize set to its actual size.
    if unsafe { GetVersionExW(&mut info) } == 0 {
        return "Microsoft Windows".to_owned();
    }
    format!(
        "Microsoft Windows {}.{}.{}",
        info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
    )
}
