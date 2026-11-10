use super::*;
use vrcr_watchdog::relay::retry::{Answer, CAPACITY, RECREATE, RetryMemory, delay, key, reason};

#[test]
fn retry_after_is_bounded_for_numbers_dates_and_bad_values() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    for (value, expected) in [
        (None, 1),
        (Some("0"), 1),
        (Some("-30"), 1),
        (Some("300"), 30),
        (Some("99999999999999999999999999999999"), 30),
        (Some("garbage"), 1),
        (Some(""), 1),
        (Some(" 12 "), 12),
    ] {
        let mut headers = http::HeaderMap::new();
        if let Some(value) = value {
            headers.insert("retry-after", value.parse().expect("header"));
        }
        for status in [502, 503, 504] {
            assert_eq!(
                delay(status, "upstream_status", &headers, now),
                Some(Duration::from_secs(expected))
            );
        }
    }
    for (offset, expected) in [(-60, 1), (0, 1), (12, 12), (300, 30)] {
        let date = if offset < 0 {
            now - Duration::from_secs(60)
        } else {
            now + Duration::from_secs(u64::try_from(offset).expect("offset"))
        };
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "retry-after",
            httpdate::fmt_http_date(date).parse().expect("date"),
        );
        assert_eq!(
            delay(503, "upstream_status", &headers, now),
            Some(Duration::from_secs(expected))
        );
    }
    let headers = http::HeaderMap::new();
    assert_eq!(
        delay(429, "upstream_status", &headers, now),
        Some(Duration::from_secs(1))
    );
    for status in [404, 410] {
        assert_eq!(delay(status, "upstream_status", &headers, now), None);
        assert_eq!(
            delay(status, "unknown_stream", &headers, now),
            Some(Duration::from_secs(5))
        );
    }
}

#[test]
fn retry_reason_accepts_only_known_header_json_and_text_values() {
    let mut headers = http::HeaderMap::new();
    for value in [
        b"stream_recreate_required".as_slice(),
        br#"{"reason":"stream_recreate_required"}"#,
    ] {
        assert_eq!(reason(&headers, value), RECREATE);
    }
    assert_eq!(
        reason(&headers, br#"{"reason":"unknown_stream"}"#),
        "unknown_stream"
    );
    assert_eq!(
        reason(&headers, b"private path?token=secret"),
        "upstream_status"
    );
    for header in ["x-vrcr-reason", "x-error-reason", "x-vrcresolver-reason"] {
        headers.clear();
        headers.insert(header, "stream_recreate_required".parse().expect("header"));
        assert_eq!(reason(&headers, b""), RECREATE);
    }
}

#[test]
fn retry_memory_is_bounded_and_request_representations_are_distinct() {
    let memory = RetryMemory::default();
    let now = SystemTime::UNIX_EPOCH;
    for index in 0..10_000 {
        memory.put(
            index.to_string(),
            Answer {
                status: http::StatusCode::SERVICE_UNAVAILABLE,
                headers: http::HeaderMap::new(),
                body: bytes::Bytes::new(),
                reason: "upstream_status",
                until: now + Duration::from_secs(30),
            },
            now,
        );
        assert!(memory.len() <= CAPACITY);
        assert!(memory.retire(&index.to_string(), now));
    }
    assert_eq!(memory.len(), CAPACITY);
    assert!(memory.get("9999", now + Duration::from_secs(29)).is_some());
    assert!(memory.get("9999", now + Duration::from_secs(30)).is_none());
    assert!(memory.is_empty());
    let mut headers = http::HeaderMap::new();
    let first = key(
        "play-one",
        "https://media.example/a.ts?private=one",
        &http::Method::GET,
        &headers,
    );
    headers.insert("range", "bytes=0-99".parse().expect("range"));
    let second = key(
        "play-one",
        "https://media.example/a.ts?private=one",
        &http::Method::GET,
        &headers,
    );
    assert_ne!(first, second);
    assert_ne!(
        second,
        key(
            "play-one",
            "https://media.example/a.ts?private=two",
            &http::Method::GET,
            &headers
        )
    );
    assert_ne!(
        second,
        key(
            "play-one",
            "https://media.example/b.ts?private=one",
            &http::Method::GET,
            &headers
        )
    );
    assert_ne!(
        second,
        key(
            "play-one",
            "https://media.example/a.ts?private=one",
            &http::Method::HEAD,
            &headers
        )
    );
    assert_eq!(first.len(), 64);
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn retry_header_limit_clear_confirmation_and_playback_isolation() {
    let memory = RetryMemory::default();
    let now = SystemTime::UNIX_EPOCH;
    let mut answer = Answer {
        status: http::StatusCode::BAD_GATEWAY,
        headers: http::HeaderMap::new(),
        body: bytes::Bytes::from_static(b"failed"),
        reason: "upstream_status",
        until: now + Duration::from_secs(30),
    };
    assert!(!memory.confirmed("key", &answer, now));
    assert!(memory.confirmed("key", &answer, now + Duration::from_millis(999)));
    memory.put("key".to_owned(), answer.clone(), now);
    assert!(memory.get("key", now).is_some());
    memory.clear("key");
    assert!(memory.get("key", now).is_none());
    assert!(!memory.confirmed("key", &answer, now));
    assert!(!memory.confirmed("key", &answer, now + Duration::from_secs(1)));
    answer
        .headers
        .insert("x", "a".repeat(16383).parse().expect("header"));
    memory.put("limit".to_owned(), answer.clone(), now);
    assert!(memory.get("limit", now).is_some());
    answer
        .headers
        .insert("x", "a".repeat(16384).parse().expect("header"));
    memory.put("oversize".to_owned(), answer, now);
    assert!(memory.get("oversize", now).is_none());
    let headers = http::HeaderMap::new();
    assert_ne!(
        key(
            "one",
            "https://media.example/a",
            &http::Method::GET,
            &headers
        ),
        key(
            "two",
            "https://media.example/a",
            &http::Method::GET,
            &headers
        )
    );
    assert_eq!(
        reason(&headers, br#"{"code":"stream_recreate_required"}"#),
        "upstream_status"
    );
    assert_eq!(
        reason(&headers, b"prefix stream_recreate_required suffix"),
        "upstream_status"
    );
}

#[test]
fn retry_named_failure_without_retry_after_is_not_treated_as_untyped() {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "x-vrcr-reason",
        "generation_failed".parse().expect("header"),
    );
    assert_eq!(
        reason(&headers, b"stream_recreate_required"),
        "upstream_failure"
    );
    let now = SystemTime::UNIX_EPOCH;
    let answer = Answer {
        status: http::StatusCode::BAD_GATEWAY,
        reason: reason(&headers, &[]),
        headers,
        body: bytes::Bytes::new(),
        until: now + Duration::from_secs(1),
    };
    assert!(RetryMemory::default().confirmed("key", &answer, now));
}
