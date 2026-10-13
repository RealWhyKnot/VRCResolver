#![allow(dead_code)]

use vrcr_shared::wrapper_identity;

const PE_OFFSET: usize = 0x80;
const DIRECTORIES: usize = PE_OFFSET + 24 + 112;

pub fn bundled(seed: usize) -> Vec<u8> {
    let len = usize::try_from(wrapper_identity::OURS_SIZE_CEILING).unwrap_or(0) + 4096;
    (0..len).map(|i| ((i * 7 + seed) & 0xFF) as u8).collect()
}

pub fn complete_bundled(seed: usize) -> Vec<u8> {
    let len = usize::try_from(wrapper_identity::OURS_SIZE_CEILING).unwrap_or(0) + 4096;
    let mut d: Vec<u8> = (0..len).map(|i| ((i * 7 + 1) & 0xFF) as u8).collect();
    d[PE_OFFSET..DIRECTORIES + 16 * 8].fill(0);
    d[0] = b'M';
    d[1] = b'Z';
    d[0x3C..0x40].copy_from_slice(&u32::try_from(PE_OFFSET).unwrap_or(0).to_le_bytes());
    d[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(b"PE\0\0");
    d[PE_OFFSET + 24..PE_OFFSET + 26].copy_from_slice(&0x20Bu16.to_le_bytes());
    let cert = u32::try_from(1024 * seed).unwrap_or(0);
    let start = u32::try_from(len).unwrap_or(0) - cert;
    d[DIRECTORIES + 32..DIRECTORIES + 36].copy_from_slice(&start.to_le_bytes());
    d[DIRECTORIES + 36..DIRECTORIES + 40].copy_from_slice(&cert.to_le_bytes());
    d
}

pub fn cut(mut d: Vec<u8>, n: usize) -> Vec<u8> {
    d.truncate(d.len() - n);
    d
}

pub fn wrapper() -> Vec<u8> {
    let mut d: Vec<u8> = (0..8192).map(|i| ((i * 31) & 0xFF) as u8).collect();
    let marker = wrapper_identity::marker();
    d[256..256 + marker.len()].copy_from_slice(&marker);
    d
}
