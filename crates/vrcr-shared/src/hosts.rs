use std::path::PathBuf;

use crate::trust_gateway::GATEWAY_HOST;

pub const MARKER_IP: &str = "127.0.0.1";

pub fn hosts_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join("drivers").join("etc").join("hosts")
}

pub fn line_is_bypass_entry(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    let body = trimmed.split('#').next().unwrap_or_default();
    let mut tokens = body.split([' ', '\t']).filter(|t| !t.is_empty());
    if tokens.next() != Some(MARKER_IP) {
        return false;
    }
    tokens.any(|t| t.eq_ignore_ascii_case(GATEWAY_HOST))
}

pub fn contains_bypass_entry(text: &str) -> bool {
    text.lines().any(line_is_bypass_entry)
}
