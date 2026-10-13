use http::HeaderMap;
use http::header::{ACCEPT_ENCODING, CONTENT_LENGTH, HeaderName, HeaderValue};

const SKIPPED_REQUEST_HEADERS: [&str; 12] = [
    "host",
    "connection",
    "cookie",
    "authorization",
    "accept-encoding",
    "content-length",
    "content-type",
    "keep-alive",
    "proxy-connection",
    "te",
    "transfer-encoding",
    "upgrade",
];

const DROPPED_RESPONSE_HEADERS: [&str; 9] = [
    "transfer-encoding",
    "alt-svc",
    "server",
    "cf-cache-status",
    "speculation-rules",
    "report-to",
    "nel",
    "connection",
    "keep-alive",
];

pub fn should_drop_response_header(name: &str) -> bool {
    DROPPED_RESPONSE_HEADERS
        .iter()
        .any(|d| d.eq_ignore_ascii_case(name))
        || name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("cf-"))
}

pub fn forward_request_headers(src: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in src {
        if SKIPPED_REQUEST_HEADERS.contains(&name.as_str()) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    out
}

pub struct CopiedHeaders {
    pub headers: HeaderMap,
    pub passed: usize,
    pub dropped: usize,
}

pub fn copy_response_headers(src: &HeaderMap, body_rewritten: bool) -> CopiedHeaders {
    let mut headers = HeaderMap::new();
    let mut passed = 0;
    let mut dropped = 0;
    for name in src.keys() {
        let n = name.as_str();
        if should_drop_response_header(n) {
            dropped += 1;
            continue;
        }
        if n == "content-length" || n == "content-type" {
            continue;
        }
        if body_rewritten && n == "content-encoding" {
            dropped += 1;
            continue;
        }
        if let Ok(v) = HeaderValue::from_bytes(&join_values(src, name)) {
            headers.insert(name.clone(), v);
            passed += 1;
        }
    }
    CopiedHeaders {
        headers,
        passed,
        dropped,
    }
}

fn join_values(src: &HeaderMap, name: &HeaderName) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, v) in src.get_all(name).iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b", ");
        }
        out.extend_from_slice(v.as_bytes());
    }
    out
}

pub fn first_header(src: &HeaderMap, name: &str) -> Option<String> {
    src.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

pub fn long_header(src: &HeaderMap, name: &str) -> i64 {
    first_header(src, name)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(-1)
}

pub fn content_length(src: &HeaderMap) -> Option<u64> {
    src.get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}
