use super::*;

#[test]
fn host_header_allows_only_expected_local_hosts() {
    for (host, port, expected) in [
        (Some("localhost.youtube.com:51234"), 51234, true),
        (Some("LOCALHOST.YOUTUBE.COM:51234"), 51234, true),
        (Some("127.0.0.1:51234"), 51234, true),
        (Some("localhost:51234"), 51234, true),
        (Some(" localhost.youtube.com "), 51234, true),
        (Some("us1.vrcresolver.com:51234"), 51234, false),
        (Some("localhost.youtube.com:51235"), 51234, false),
        (Some(""), 51234, false),
        (None, 51234, false),
    ] {
        assert_eq!(is_allowed_host_header(host, port), expected, "{host:?}");
    }
}

#[test]
fn response_header_drop_list_matches() {
    for name in [
        "Transfer-Encoding",
        "alt-svc",
        "Server",
        "cf-cache-status",
        "Speculation-Rules",
        "Report-To",
        "NEL",
        "CF-RAY",
        "cf-connecting-ip",
        "Connection",
    ] {
        assert!(should_drop_response_header(name), "{name}");
    }
    for name in [
        "Content-Range",
        "Accept-Ranges",
        "Cache-Control",
        "ETag",
        "cfx",
    ] {
        assert!(!should_drop_response_header(name), "{name}");
    }
}

#[test]
fn request_headers_forward_everything_but_credentials_and_framing() {
    let mut src = HeaderMap::new();
    for (k, v) in [
        ("host", "localhost.youtube.com:1"),
        ("cookie", "a=b"),
        ("authorization", "Bearer x"),
        ("accept-encoding", "gzip"),
        ("connection", "keep-alive"),
        ("content-length", "0"),
        ("content-type", "text/plain"),
        ("range", "bytes=10-"),
        ("user-agent", "AVProMobileVideo"),
    ] {
        src.insert(k, HeaderValue::from_static(v));
    }
    let out = forward_request_headers(&src);
    let names: Vec<&str> = out.keys().map(http::HeaderName::as_str).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(out["range"], "bytes=10-");
    assert_eq!(out["user-agent"], "AVProMobileVideo");
    assert_eq!(out["accept-encoding"], "identity");
}

#[test]
fn copied_response_headers_join_values_and_drop_rewritten_encoding() {
    let mut src = HeaderMap::new();
    src.append("set-thing", HeaderValue::from_static("a"));
    src.append("set-thing", HeaderValue::from_static("b"));
    src.insert("content-encoding", HeaderValue::from_static("gzip"));
    src.insert("content-length", HeaderValue::from_static("10"));
    src.insert("content-type", HeaderValue::from_static("video/mp2t"));
    src.insert("cf-ray", HeaderValue::from_static("x"));
    let kept = copy_response_headers(&src, false);
    assert_eq!(kept.headers["set-thing"], "a, b");
    assert_eq!(kept.headers["content-encoding"], "gzip");
    assert!(!kept.headers.contains_key("content-length"));
    assert!(!kept.headers.contains_key("content-type"));
    assert_eq!((kept.passed, kept.dropped), (2, 1));
    let rewritten = copy_response_headers(&src, true);
    assert!(!rewritten.headers.contains_key("content-encoding"));
    assert_eq!((rewritten.passed, rewritten.dropped), (1, 2));
}
