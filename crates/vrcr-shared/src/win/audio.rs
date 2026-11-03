use windows_sys::Win32::Media::Audio::{
    HWAVEOUT, WAVEFORMATEX, WAVEOUTCAPSW, waveOutClose, waveOutGetDevCapsW, waveOutGetNumDevs,
    waveOutMessage, waveOutOpen,
};
use windows_sys::Win32::Media::Multimedia::DRVM_MAPPER_PREFERRED_GET;

use super::DefaultOutput;

const NO_PREFERRED_DEVICE: u32 = u32::MAX;

pub fn probe_default_output() -> DefaultOutput {
    // SAFETY: waveOutGetNumDevs takes no pointers and reads device availability.
    if unsafe { waveOutGetNumDevs() } == 0 {
        return DefaultOutput::NoDevices;
    }
    let mut device_id: u32 = 0;
    let mut status: u32 = 0;
    let mapper: HWAVEOUT = std::ptr::without_provenance_mut(usize::MAX);
    // SAFETY: The mapper sentinel is valid for this message and both integer output pointers remain live.
    let r = unsafe {
        waveOutMessage(
            mapper,
            DRVM_MAPPER_PREFERRED_GET,
            std::ptr::from_mut(&mut device_id).expose_provenance(),
            std::ptr::from_mut(&mut status).expose_provenance(),
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
    // SAFETY: format is a complete PCM descriptor, handle is writable, and no callback is registered.
    let result = unsafe { waveOutOpen(&mut handle, device_id, &format, 0, 0, 0) };
    if result == 0 {
        // SAFETY: A successful waveOutOpen returned this handle, which is closed exactly once.
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
    // SAFETY: caps is writable for size bytes and device_id is a scalar device selector.
    if unsafe {
        waveOutGetDevCapsW(
            usize::try_from(device_id).expect("Windows pointer width holds a device ID"),
            &mut caps,
            size,
        )
    } != 0
    {
        return None;
    }
    let raw = caps.szPname;
    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let name = String::from_utf16_lossy(&raw[..len]);
    (!name.is_empty()).then_some(name)
}
