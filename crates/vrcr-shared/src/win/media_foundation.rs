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
        // SAFETY: The SDK version and startup flags are valid scalar constants.
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
        // SAFETY: input is a live descriptor and activates and count are distinct writable output pointers.
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
            for i in
                0..usize::try_from(count).expect("Windows pointer width holds the transform count")
            {
                // SAFETY: MFTEnumEx returned an array of count interface pointers and i is below count.
                let p = unsafe { *activates.add(i) };
                if !p.is_null() {
                    // SAFETY: A non-null IMFActivate interface starts with a pointer to its IUnknown-compatible vtable.
                    let vtbl = unsafe { *p.cast::<*const Release>() };
                    // SAFETY: The first three IUnknown vtable slots exist and slot two is the system-ABI Release method.
                    let release = unsafe { *vtbl.add(2) };
                    // SAFETY: release is this interface vtable method and p still owns the enumeration reference.
                    unsafe { release(p) };
                }
            }
            // SAFETY: MFTEnumEx allocated the array with CoTaskMemAlloc and each interface reference was released.
            unsafe { CoTaskMemFree(activates.cast()) };
        }
        Some(count > 0)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: This instance exists only after a successful MFStartup and balances that startup once.
        unsafe { MFShutdown() };
    }
}
