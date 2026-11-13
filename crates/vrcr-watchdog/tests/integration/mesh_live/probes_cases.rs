use super::*;

#[tokio::test]
async fn resolve_on_a_silent_connection_reconnects_and_resends() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 {
            Mode::Blackhole
        } else {
            Mode::Accept
        }
    })))
    .await;
    let t = probing(
        &server,
        Duration::from_millis(300),
        Duration::from_millis(300),
    );
    t.client.start();

    let started = std::time::Instant::now();
    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(server.connections(), 2);
    let resolves = server.resolves();
    assert!(resolves.contains(&(1, "r1".to_owned())));
    assert!(resolves.contains(&(2, "r1".to_owned())));
    t.client.stop().await;
}

#[tokio::test]
async fn dropping_a_silent_connection_prints_one_console_warning() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 {
            Mode::Blackhole
        } else {
            Mode::Accept
        }
    })))
    .await;
    let t = probing(
        &server,
        Duration::from_millis(300),
        Duration::from_millis(300),
    );
    t.client.start();

    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    let out = t.out.logs();
    assert_eq!(out.matches("disconnected (error)").count(), 1, "{out}");
    assert!(!out.contains("no answer to a ping"), "{out}");
    assert!(
        t.log()
            .contains("no answer to a ping within 0.3s while a resolve waited")
    );
    t.client.stop().await;
}

#[tokio::test]
async fn slow_but_alive_server_keeps_its_connection() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).on_resolve(handler(
        |c: Conn, id: String| async move {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(1_500)).await;
                c.resolved(&id);
            });
        },
    )))
    .await;
    let t = probing(
        &server,
        Duration::from_millis(300),
        Duration::from_millis(800),
    );
    t.client.start();

    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert_eq!(server.connections(), 1);
    assert!(server.record(|r| !r.pings.is_empty()));
    t.client.stop().await;
}

#[tokio::test]
async fn two_resolves_on_a_silent_link_are_each_resent_once() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 {
            Mode::Blackhole
        } else {
            Mode::Accept
        }
    })))
    .await;
    let t = probing(
        &server,
        Duration::from_millis(300),
        Duration::from_millis(300),
    );
    t.client.start();

    let (a, b) = tokio::join!(
        resolve(&t.client, "first", Duration::from_secs(10)),
        resolve(&t.client, "second", Duration::from_secs(10))
    );

    assert_eq!(a.action, action::RESOLVED);
    assert_eq!(b.action, action::RESOLVED);
    let resolves = server.resolves();
    let count = |c: u32, id: &str| resolves.iter().filter(|r| r.0 == c && r.1 == id).count();
    assert_eq!(count(1, "first"), 1);
    assert_eq!(count(1, "second"), 1);
    assert_eq!(count(2, "first"), 1);
    assert_eq!(count(2, "second"), 1);
    assert_eq!(
        server.record(|r| r.pings.iter().filter(|c| **c == 1).count()),
        1
    );
    t.client.stop().await;
}

#[tokio::test]
async fn hung_upgrade_is_reported_with_its_phase_and_the_next_probe_starts_fresh() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 { Mode::Hang } else { Mode::Accept }
    })))
    .await;
    let t = client_for(&server, |c| c.connect_deadline = Duration::from_millis(500));
    t.client.start();

    wait_until(
        || t.client.is_connected(),
        10_000,
        "mesh connection after a hung first connect",
    )
    .await;

    let out = t.out.logs();
    assert!(
        out.contains("connect to vrcresolver.com timed out after"),
        "{out}"
    );
    assert!(out.contains("during upgrade"), "{out}");
    assert!(!out.contains("/mesh"), "{out}");
    assert_eq!(server.attempts(), 2);
    let probe = t.client.last_connect_probe();
    assert_eq!(probe.phase, Phase::Upgrade);
    assert!(probe.tcp_ms.is_some());
    t.client.stop().await;
}
