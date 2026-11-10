use super::*;

#[tokio::test]
async fn rate_limited_resolve_with_id_fails_that_pending_only() {
    let h = Harness::new();
    let mut limited = h.pending("req1");
    let mut other = h.pending("req2");

    h.json(r#"{"action":"rate_limited","meshAction":"resolve","limit":10,"windowSeconds":60,"retryAfterSeconds":6,"id":"req1"}"#)
        .await;

    let result = done(&mut limited).unwrap_or_else(|| panic!("req1 still pending"));
    assert_eq!(result.reason.as_deref(), Some(fallback::RATE_LIMITED));
    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert!(done(&mut other).is_none());
    assert!(h.cooldown().is_some_and(|t| t > Instant::now()));
}

#[tokio::test]
async fn rate_limited_resolve_without_id_fails_all_pending() {
    let h = Harness::new();
    let mut a = h.pending("a");
    let mut b = h.pending("b");

    h.json(r#"{"action":"rate_limited","meshAction":"resolve","limit":10,"windowSeconds":60,"retryAfterSeconds":6}"#)
        .await;

    let a = done(&mut a).unwrap_or_else(|| panic!("a still pending"));
    assert!(done(&mut b).is_some());
    assert_eq!(a.reason.as_deref(), Some(fallback::RATE_LIMITED));
    assert!(
        h.out
            .text()
            .contains("failing 2 pending requests reason=rate_limited ids=a,b")
    );
}

#[tokio::test]
async fn resolve_short_circuits_during_the_cooldown_without_wire_traffic() {
    let h = Harness::new();
    *super::super::lock(&h.client.inner.rate_limited_until) =
        Some(Instant::now() + Duration::from_secs(30));
    let req = ResolveRequest {
        url: "https://example.com/v".to_owned(),
        ..ResolveRequest::default()
    };

    let result = h
        .client
        .resolve(&req, Instant::now() + Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert_eq!(result.reason.as_deref(), Some(fallback::RATE_LIMITED));
    let frame: ResolveResponse =
        serde_json::from_slice(&result.frame).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(frame.reason.as_deref(), Some(fallback::RATE_LIMITED));
    assert_eq!(frame.id.len(), 32);
    assert!(super::super::lock(&h.client.inner.pending).is_empty());
}

#[tokio::test]
async fn resolved_url_remembers_the_correlation_id_for_later_feedback() {
    let h = Harness::new();
    let _rx = h.pending("r1");
    super::super::lock(&h.client.inner.inflight_cids).insert("r1".to_owned(), "cid-1".to_owned());

    h.json(r#"{"action":"resolved","id":"r1","url":"https://media.example/v.mp4"}"#)
        .await;

    assert_eq!(
        h.client
            .inner
            .lookup_recent_cid("https://media.example/v.mp4")
            .as_deref(),
        Some("cid-1")
    );
    assert!(super::super::lock(&h.client.inner.inflight_cids).is_empty());
}

#[tokio::test]
async fn recent_correlation_ids_are_capped_and_expire() {
    let h = Harness::new();
    let inner = &h.client.inner;
    for i in 0..=super::super::MAX_RECENT_CIDS {
        inner.remember_resolved_url_cid(&format!("https://m.example/{i}"), &format!("c{i}"));
    }

    assert_eq!(
        super::super::lock(&inner.recent_cids).len(),
        super::super::MAX_RECENT_CIDS
    );
    assert!(inner.lookup_recent_cid("https://m.example/0").is_none());
    assert_eq!(
        inner
            .lookup_recent_cid(&format!(
                "https://m.example/{}",
                super::super::MAX_RECENT_CIDS
            ))
            .as_deref(),
        Some(format!("c{}", super::super::MAX_RECENT_CIDS).as_str())
    );

    if let Some(old) = Instant::now().checked_sub(RECENT_CIDS_TTL + Duration::from_secs(1)) {
        super::super::lock(&inner.recent_cids).insert(
            "https://m.example/old".to_owned(),
            ("c-old".to_owned(), old),
        );
        assert!(inner.lookup_recent_cid("https://m.example/old").is_none());
    }
}

#[tokio::test]
async fn failing_many_pending_requests_lists_only_the_first_eight_ids() {
    let h = Harness::new();
    let mut receivers = Vec::new();
    for i in 0..10 {
        receivers.push(h.pending(&format!("id{i:02}")));
    }

    h.client
        .inner
        .fail_all_pending(fallback::SERVER_UNREACHABLE);

    assert!(receivers.iter_mut().all(|rx| done(rx).is_some()));
    assert!(h.out.text().contains(
        "failing 10 pending requests reason=server_unreachable ids=id00,id01,id02,id03,id04,id05,id06,id07,...(+2)"
    ));
}

#[tokio::test]
async fn resolve_log_goes_to_the_file_only() {
    let h = Harness::new();

    h.json(r#"{"action":"resolve_log","id":"r1","level":"warn","message":"trying tv"}"#)
        .await;

    assert_eq!(h.out.text(), "");
    assert!(
        h.log()
            .contains("[mesh][resolve_log] id=r1 level=warn phase=trying config=tv")
    );
}

#[tokio::test]
async fn resolve_during_a_long_outage_fails_fast() {
    let h = Harness::new();
    let Some(long_ago) = Instant::now().checked_sub(Duration::from_secs(120)) else {
        return;
    };
    *super::super::lock(&h.client.inner.link_down_since) = long_ago;
    let req = ResolveRequest {
        id: "r1".to_owned(),
        url: "https://www.youtube.com/watch?v=abc".to_owned(),
        ..ResolveRequest::default()
    };

    let started = std::time::Instant::now();
    let result = h
        .client
        .resolve(&req, Instant::now() + Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(super::super::lock(&h.client.inner.pending).is_empty());
    assert!(super::super::lock(&h.client.inner.inflight_cids).is_empty());
}

#[tokio::test]
async fn feedback_without_a_link_is_dropped() {
    let h = Harness::new();
    h.client.send_playback_feedback(crate::ipc::Feedback {
        url: "https://example.com/v".to_owned(),
        kind: vrcr_shared::wire::feedback::PLAYING,
        ms_since_open: 10,
        delivered_height: None,
        detail: None,
        correlation_id: None,
    });
    assert!(!h.client.is_connected());
    drop(h.dir);
}
