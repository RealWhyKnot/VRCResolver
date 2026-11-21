use super::*;
use tokio::net::windows::named_pipe::ClientOptions;

#[tokio::test]
async fn a_missing_pipe_fails_at_once_without_a_retry() {
    let rig = Rig::new();
    let started = Instant::now();
    let out = rig
        .shim()
        .resolve_over_pipe(URL, Player::AvPro, None, None, false)
        .await;
    let elapsed = started.elapsed();
    assert_eq!(out.fallback_reason, Some(og_reason::PIPE_CONNECT_FAILED));
    let log = rig.log();
    assert!(log.contains("pipe connect ENOENT"), "{log}");
    assert!(!log.contains("pipe connect retry"), "{log}");
    assert!(elapsed < Duration::from_millis(750), "{elapsed:?}");
}

#[tokio::test]
async fn pipe_connect_reports_not_found_when_nothing_listens() {
    let name = format!("vrcresolver.test.{}", uuid::Uuid::new_v4().simple());
    let result = pipe::connect(&name, pipe::CONNECT_TIMEOUT).await;
    assert!(
        matches!(result, Err(pipe::ConnectError::NotFound)),
        "{result:?}"
    );
}

#[tokio::test]
async fn connect_retry_reaches_a_busy_watchdog_and_notifies_still_go_out() {
    let rig = Rig::new();
    let full = pipe::full_name(&rig.pipe_name);
    let busy = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&full)
        .expect("local test fixture operation must succeed");
    let hog = ClientOptions::new()
        .open(&full)
        .expect("local test fixture operation must succeed");
    busy.connect()
        .await
        .expect("local test fixture operation must succeed");
    let reply = r#"{"action":"fallback_native","id":"srv1","reason":"resolve_failed"}"#;
    let mut server = StubPipe::start(&rig.pipe_name, Some(reply), Duration::from_millis(1300));
    let mut shim = rig.shim();
    let out = shim
        .resolve_over_pipe(URL, Player::AvPro, None, None, false)
        .await;
    assert_eq!(out.fallback_reason, Some(og_reason::SERVER_FALLBACK_NATIVE));
    assert_eq!(server.next_line().await["action"], "resolve");
    shim.notify_og_fallback(Some(URL), og_reason::SERVER_FALLBACK_NATIVE, 2000, None)
        .await;
    assert_eq!(server.next_line().await["action"], "og_fallback_notify");
    let log = rig.log();
    assert!(log.contains("pipe connect TIMED OUT"), "{log}");
    assert!(log.contains("pipe reconnect OK retry=1"), "{log}");
    drop((hog, busy));
}
