use super::*;

#[tokio::test]
async fn stop_fails_waiting_resolves_and_closes_the_link() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).silent_resolve()).await;
    let t = connected(&server).await;
    let c = t.client.clone();
    let pending = tokio::spawn(async move { resolve(&c, "r1", Duration::from_secs(20)).await });
    wait_until(
        || !server.resolves().is_empty(),
        5_000,
        "resolve on the wire",
    )
    .await;

    t.client.stop().await;
    let result = pending.await.unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    assert!(
        t.out
            .logs()
            .contains("failing 1 pending requests reason=server_unreachable ids=r1")
    );
    assert!(!t.client.is_connected());
}
