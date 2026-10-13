use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::legacy;

const MARKER_HEAD: &str = "VRCRESOLVER_WRAPPER_MARKER_v1:";
const MARKER_TAIL: &str = "6f2a9c41-8d5e-4b7a-a3c9-1e8f7d2b4a60";
pub const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;
pub const OURS_SIZE_CEILING: u64 = 10 * 1024 * 1024;
const COOKIE_SEARCH_BYTES: u64 = 4096;
pub const COMPANY_NAME: &str = "RealWhyKnot";
pub const PRODUCT_NAME: &str = "VRCResolver";
pub const PYINSTALLER_COOKIE: &[u8] = b"MEI\x0C\x0B\x0A\x0B\x0E";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapperKind {
    Ours,
    VrcBundledYtDlp,
    Unknown,
}

pub fn marker() -> Vec<u8> {
    [MARKER_HEAD.as_bytes(), MARKER_TAIL.as_bytes()].concat()
}

pub fn looks_complete(path: &Path) -> bool {
    check_complete(path).unwrap_or(false)
}

fn check_complete(path: &Path) -> Option<bool> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let pe_offset = u64::from(u32::from_le_bytes(read_at(&mut file, length, 0x3C)?));
    if read_at::<4>(&mut file, length, pe_offset)? != *b"PE\0\0" {
        return Some(false);
    }
    let directories = match u16::from_le_bytes(read_at(&mut file, length, pe_offset + 24)?) {
        0x10B => pe_offset + 24 + 96,
        0x20B => pe_offset + 24 + 112,
        _ => return Some(false),
    };
    let security: [u8; 8] = read_at(&mut file, length, directories + 32)?;
    let cert_offset = u64::from(u32::from_le_bytes([
        security[0],
        security[1],
        security[2],
        security[3],
    ]));
    let cert_size = u64::from(u32::from_le_bytes([
        security[4],
        security[5],
        security[6],
        security[7],
    ]));
    if cert_size > 0 {
        return Some(cert_offset + cert_size == length);
    }
    let tail_len = length.min(COOKIE_SEARCH_BYTES);
    let mut tail = vec![0u8; usize::try_from(tail_len).ok()?];
    file.seek(SeekFrom::Start(length - tail_len)).ok()?;
    file.read_exact(&mut tail).ok()?;
    Some(find(&tail, PYINSTALLER_COOKIE))
}

fn read_at<const N: usize>(file: &mut File, length: u64, offset: u64) -> Option<[u8; N]> {
    if offset.checked_add(N as u64)? > length {
        return None;
    }
    let mut buf = [0u8; N];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

pub fn classify(path: &Path, known_hashes: Option<&Path>) -> WrapperKind {
    let Ok(meta) = fs::metadata(path) else {
        return WrapperKind::Unknown;
    };
    if !meta.is_file() {
        return WrapperKind::Unknown;
    }
    let size = meta.len();
    if size <= MAX_SCAN_BYTES && contains_marker(path) {
        return WrapperKind::Ours;
    }
    if let Some(info) = version::query(path)
        && info.company.as_deref() == Some(COMPANY_NAME)
        && matches!(info.product.as_deref(), Some(PRODUCT_NAME | legacy::PRODUCT_NAME))
    {
        return WrapperKind::Ours;
    }
    if let Some(list) = known_hashes.filter(|p| p.is_file())
        && sha256_hex(path).is_some_and(|sha| known_hash_list_contains(list, &sha))
    {
        return WrapperKind::Ours;
    }
    if size > OURS_SIZE_CEILING {
        WrapperKind::VrcBundledYtDlp
    } else {
        WrapperKind::Unknown
    }
}

pub fn contains_marker(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_SCAN_BYTES {
        return false;
    }
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    find(&bytes, &marker()) || find(&bytes, &legacy::wrapper_marker())
}

pub fn sha256_hex(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex::encode(hasher.finalize()))
}

pub fn known_hash_list_contains(list: &Path, sha256: &str) -> bool {
    let Ok(text) = fs::read_to_string(list) else {
        return false;
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .any(|l| {
            l.split(' ')
                .next()
                .is_some_and(|head| head.eq_ignore_ascii_case(sha256))
        })
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.len() >= needle.len() && hay.windows(needle.len()).any(|w| w == needle)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionInfo {
    pub company: Option<String>,
    pub product: Option<String>,
    pub file_version: Option<String>,
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod version {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    use super::VersionInfo;

    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn query_raw<'a>(block: &'a [u8], sub: &str) -> Option<&'a [u8]> {
        let sub = wide(sub);
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len: u32 = 0;
        let ok = unsafe { VerQueryValueW(block.as_ptr().cast(), sub.as_ptr(), &mut ptr, &mut len) };
        if ok == 0 || ptr.is_null() {
            return None;
        }
        let start = block.as_ptr() as usize;
        let at = ptr as usize;
        let end = start + block.len();
        if at < start || at >= end {
            return None;
        }
        let avail = end - at;
        Some(unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), avail) })
    }

    fn query_string(block: &[u8], lang: &str, key: &str) -> Option<String> {
        let raw = query_raw(block, &format!("\\StringFileInfo\\{lang}\\{key}"))?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|u| *u != 0)
            .collect();
        let s = String::from_utf16_lossy(&units);
        (!s.is_empty()).then_some(s)
    }

    fn languages(block: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let sub = wide("\\VarFileInfo\\Translation");
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len: u32 = 0;
        let ok = unsafe { VerQueryValueW(block.as_ptr().cast(), sub.as_ptr(), &mut ptr, &mut len) };
        if ok != 0 && !ptr.is_null() {
            let start = block.as_ptr() as usize;
            let at = ptr as usize;
            let bytes = (len as usize).min((start + block.len()).saturating_sub(at));
            if at >= start {
                let raw = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), bytes) };
                for [a, b, c, d] in raw.as_chunks::<4>().0 {
                    let lang = u16::from_le_bytes([*a, *b]);
                    let cp = u16::from_le_bytes([*c, *d]);
                    out.push(format!("{lang:04X}{cp:04X}"));
                }
            }
        }
        for fallback in ["040904B0", "040904E4", "04090000"] {
            if !out.iter().any(|l| l == fallback) {
                out.push(fallback.to_owned());
            }
        }
        out
    }

    pub fn query(path: &Path) -> Option<VersionInfo> {
        let name = wide(path.as_os_str());
        let mut handle = 0u32;
        let size = unsafe { GetFileVersionInfoSizeW(name.as_ptr(), &mut handle) };
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        let ok = unsafe { GetFileVersionInfoW(name.as_ptr(), 0, size, block.as_mut_ptr().cast()) };
        if ok == 0 {
            return None;
        }
        let langs = languages(&block);
        let pick = |key: &str| langs.iter().find_map(|l| query_string(&block, l, key));
        Some(VersionInfo {
            company: pick("CompanyName"),
            product: pick("ProductName"),
            file_version: pick("FileVersion"),
        })
    }
}

#[cfg(not(windows))]
pub mod version {
    use std::path::Path;

    use super::VersionInfo;

    pub fn query(_path: &Path) -> Option<VersionInfo> {
        None
    }
}
