use super::*;

#[tokio::test]
async fn deadline_ends_a_resolve_while_its_probe_waits() {
    let server = Server::start(Options::new(policy(|_| Mode::Blackhole))).await;
    let t = probing(&server, Duration::from_millis(300), Duration::from_secs(30));
    t.client.start();
    wait_until(|| t.client.is_connected(), 5_000, "connected").await;

    let started = std::time::Instant::now();
    let c = t.client.clone();
    let pending =
        tokio::spawn(async move { resolve(&c, "r1", Duration::from_millis(1_500)).await });
    wait_until(
        || server.record(|r| !r.pings.is_empty()),
        5_000,
        "stall probe ping",
    )
    .await;
    let result = pending.await.unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(
        result.reason.as_deref(),
        Some(fallback::CLIENT_DEADLINE_EXCEEDED)
    );
    let took = ms(started.elapsed());
    assert!((1_400..2_500).contains(&took), "took {took} ms");
    t.client.stop().await;
}

#[tokio::test]
async fn retry_after_discovery_in_progress_sends_a_smaller_wrapper_deadline() {
    let answers = Arc::new(AtomicU32::new(0));
    let n = Arc::clone(&answers);
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).on_resolve(handler(
        move |c: Conn, id: String| {
            let n = Arc::clone(&n);
            async move {
                if n.fetch_add(1, Ordering::SeqCst) == 0 {
                    c.text(format!(
                        r#"{{"action":"fallback_native","id":"{id}","reason":"discovery_in_progress","retry_after_ms":800}}"#
                    ));
                } else {
                    c.resolved(&id);
                }
            }
        },
    )))
    .await;
    let t = connected(&server).await;

    let reply = ipc_round_trip(&t.client, &request_line(Some(20_000))).await;

    assert_eq!(reply["action"], action::RESOLVED);
    let sent: Vec<i64> =
        server.record(|r| r.request_json.iter().map(|j| sent_deadline(j)).collect());
    assert_eq!(sent.len(), 2);
    assert!((17_000..=19_000).contains(&sent[0]), "{sent:?}");
    assert!(sent[1] <= sent[0] - 700, "{sent:?}");
    t.client.stop().await;
}

#[tokio::test]
async fn legacy_request_without_a_wrapper_deadline_tells_the_server_less_than_fifteen_seconds() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept))).await;
    let t = connected(&server).await;

    let reply = ipc_round_trip(&t.client, &request_line(None)).await;

    assert_eq!(reply["action"], action::RESOLVED);
    let sent: Vec<i64> =
        server.record(|r| r.request_json.iter().map(|j| sent_deadline(j)).collect());
    assert_eq!(sent.len(), 1);
    assert!((10_000..=14_500).contains(&sent[0]), "{sent:?}");
    t.client.stop().await;
}

#[tokio::test]
async fn short_wrapper_budget_is_honoured() {
    for budget in [1_500, 0] {
        let server = Server::start(Options::new(policy(|_| Mode::Accept)).silent_resolve()).await;
        let t = connected(&server).await;

        let started = std::time::Instant::now();
        let reply = ipc_round_trip(&t.client, &request_line(Some(budget))).await;

        assert_eq!(reply["action"], action::FALLBACK_NATIVE);
        assert_eq!(reply["reason"], fallback::CLIENT_DEADLINE_EXCEEDED);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "took {:?}",
            started.elapsed()
        );
        t.client.stop().await;
    }
}
