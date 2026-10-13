pub const WATCHDOG_MUTEX: &str = "Global\\WKVRCProxy.Watchdog";
pub const WATCHDOG_MUTEX_LOCAL: &str = "Local\\WKVRCProxy.Watchdog";
pub const PRODUCT_NAME: &str = "WKVRCProxy";
pub const STATE_DIR_NAME: &str = "WKVRCProxy";
pub const PIPE_NAME: &str = "WKVRCProxy.resolve";

const MARKER_HEAD: &str = "WKVRCPROXY_WRAPPER_MARKER_v1:";
const MARKER_TAIL: &str = "9b3e7c8a-7f23-4e6b-9c1d-a4f8e0d2c5b6";

pub fn wrapper_marker() -> Vec<u8> {
    [MARKER_HEAD.as_bytes(), MARKER_TAIL.as_bytes()].concat()
}

pub fn env_with_fallback(suffix: &str) -> Option<String> {
    env_with_fallback_in(suffix, |k| std::env::var(k).ok())
}

pub fn env_with_fallback_in(suffix: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    get(&format!("VRCRESOLVER_{suffix}")).or_else(|| get(&format!("WKVRCPROXY_{suffix}")))
}
