mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::mesh::{
    Conn, MEDIA, Mode, Options, Server, client_for, client_in, fixed_backoff, handler, policy, wait_until,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use vrcr_shared::console::Ux;
use vrcr_shared::log::Log;
use vrcr_shared::wire::{ResolveRequest, action, fallback, feature, feedback};
use vrcr_watchdog::ipc::{Feedback, Ipc, Mesh, MeshResult};
use vrcr_watchdog::mesh::{MeshClient, PROXY_HOST, connect::Phase};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn request(id: &str) -> ResolveRequest {
    ResolveRequest {
        id: id.to_owned(),
        url: "https://www.youtube.com/watch?v=abc".to_owned(),
        player: Some(vrcr_shared::wire::Player::AvPro),
        ..ResolveRequest::default()
    }
}

async fn resolve(client: &MeshClient, id: &str, within: Duration) -> MeshResult {
    client
        .resolve(&request(id), Instant::now() + within)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

#[tokio::test]
async fn resolve_during_a_long_backoff_wakes_the_reconnect_loop() {
    let allow = Arc::new(AtomicU32::new(0));
    let a = Arc::clone(&allow);
    let server = Server::start(Options::new(policy(move |_| {
        if a.load(Ordering::SeqCst) == 1 {
            Mode::Accept
        } else {
            Mode::Refuse
        }
    })))
    .await;
    let t = client_for(&server, |c| {
        c.reconnect_grace = Duration::from_secs(10);
        c.next_backoff_ms = fixed_backoff(30_000);
    });
    t.client.start();
    wait_until(|| server.attempts() >= 1, 5_000, "first refused attempt").await;
    allow.store(1, Ordering::SeqCst);

    let started = std::time::Instant::now();
    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_waits_for_a_connect_already_in_flight_past_the_grace() {
    let mut opts = Options::new(policy(|a| if a == 1 { Mode::Refuse } else { Mode::SlowAccept }));
    opts.slow_accept = Duration::from_secs(2);
    let server = Server::start(opts).await;
    let t = client_for(&server, |c| {
        c.connect_deadline = Duration::from_secs(5);
        c.reconnect_grace = Duration::from_secs(1);
        c.next_backoff_ms = fixed_backoff(100);
    });
    t.client.start();

    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert_eq!(server.attempts(), 2);
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_asked_while_a_connect_hangs_is_served_by_the_retry_that_follows_it() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 { Mode::Hang } else { Mode::Accept }
    })))
    .await;
    let t = client_for(&server, |c| {
        c.connect_deadline = Duration::from_millis(1_200);
        c.reconnect_grace = Duration::from_secs(10);
        c.next_backoff_ms = fixed_backoff(30_000);
    });
    t.client.start();
    wait_until(|| server.attempts() >= 1, 5_000, "first hanging attempt").await;

    let started = std::time::Instant::now();
    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?}",
        started.elapsed()
    );
    t.client.stop().await;
}

#[tokio::test]
async fn a_wake_left_over_from_an_earlier_resolve_does_not_skip_the_next_backoff() {
    let mut opts = Options::new(policy(|a| if a == 1 { Mode::SlowAccept } else { Mode::Accept }));
    opts.slow_accept = Duration::from_secs(1);
    let server = Server::start(opts).await;
    let t = client_for(&server, |c| {
        c.reconnect_grace = Duration::from_secs(10);
        c.next_backoff_ms = fixed_backoff(3_000);
    });
    t.client.start();
    wait_until(|| server.attempts() >= 1, 5_000, "first slow attempt").await;
    let first = resolve(&t.client, "r1", Duration::from_secs(20)).await;
    assert_eq!(first.action, action::RESOLVED);

    t.client.drop_connection();
    wait_until(|| !t.client.is_connected(), 5_000, "dropped link").await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    assert_eq!(server.attempts(), 1);
    wait_until(|| server.attempts() >= 2, 6_000, "reconnect after the backoff").await;
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_stops_waiting_once_the_in_flight_connect_gives_up() {
    let server = Server::start(Options::new(policy(|_| Mode::Hang))).await;
    let t = client_for(&server, |c| {
        c.connect_deadline = Duration::from_millis(1_500);
        c.reconnect_grace = Duration::from_secs(1);
        c.next_backoff_ms = fixed_backoff(30_000);
    });
    t.client.start();
    wait_until(|| server.attempts() >= 1, 5_000, "first hanging attempt").await;

    let started = std::time::Instant::now();
    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    let took = ms(started.elapsed());
    assert!((1_100..4_000).contains(&took), "took {took} ms");
    t.client.stop().await;
}

#[tokio::test]
async fn a_burst_of_resolves_during_a_backoff_causes_one_extra_attempt() {
    let stamps = Arc::new(Mutex::new(Vec::new()));
    let s = Arc::clone(&stamps);
    let clock = std::time::Instant::now();
    let mut opts = Options::new(policy(|_| Mode::Refuse));
    opts.on_attempt = Some(Arc::new(move |_| lock(&s).push(ms(clock.elapsed()))));
    let server = Server::start(opts).await;
    let t = client_for(&server, |c| {
        c.reconnect_grace = Duration::from_millis(2_500);
        c.next_backoff_ms = fixed_backoff(30_000);
    });
    t.client.start();
    wait_until(|| server.attempts() >= 1, 5_000, "first refused attempt").await;

    let mut joins = Vec::new();
    for i in 0..10 {
        let c = t.client.clone();
        joins.push(tokio::spawn(async move {
            resolve(&c, &format!("b{i}"), Duration::from_secs(20)).await
        }));
    }
    for j in joins {
        let r = j.await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    }

    let times = lock(&stamps).clone();
    assert_eq!(times.len(), 2, "{times:?}");
    assert!(times[1] - times[0] >= 500, "{times:?}");
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_in_flight_at_a_drop_is_resent_on_the_next_connection() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).on_resolve(handler(
        |c: Conn, id: String| async move {
            if c.index == 1 {
                c.abort();
            } else {
                c.resolved(&id);
            }
        },
    )))
    .await;
    let t = client_for(&server, |c| c.reconnect_grace = Duration::from_secs(5));
    t.client.start();
    wait_until(|| t.client.is_connected(), 5_000, "connected").await;

    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(
        result.action,
        action::RESOLVED,
        "{:?}
{}
{}",
        result.reason,
        t.out.text(),
        t.log()
    );
    assert_eq!(
        server.resolves(),
        vec![(1, "r1".to_owned()), (2, "r1".to_owned())]
    );
    assert!(
        t.log()
            .contains("connection dropped with id=r1 pending; resending after reconnect")
    );
    assert!(t.log().contains("resent id=r1 after reconnect (send 2)"));
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_asked_while_reconnecting_waits_for_the_new_connection() {
    let mut opts = Options::new(policy(|_| Mode::Accept));
    opts.gate_later_connections = true;
    opts.on_connect = Some(handler(|c: Conn, _id: String| async move {
        if c.index == 1 {
            c.abort();
        }
    }));
    let server = Server::start(opts).await;
    let t = client_for(&server, |c| c.reconnect_grace = Duration::from_secs(5));
    t.client.start();
    wait_until(|| server.connections() >= 1, 5_000, "first connection").await;
    wait_until(|| !t.client.is_connected(), 5_000, "dropped link").await;

    let c = t.client.clone();
    let pending = tokio::spawn(async move { resolve(&c, "r1", Duration::from_secs(20)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!pending.is_finished());
    assert_eq!(server.connections(), 1);
    server.open_gate();
    let result = pending.await.unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.action, action::RESOLVED);
    assert_eq!(server.resolves()[0].0, 2);
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_fails_to_og_once_the_reconnect_grace_runs_out() {
    let server = Arc::new(Server::start(Options::new(policy(|_| Mode::Accept)).silent_resolve()).await);
    let t = client_for(&server, |c| c.reconnect_grace = Duration::from_millis(1_500));
    t.client.start();
    wait_until(|| t.client.is_connected(), 5_000, "connected").await;
    server.stop_accepting();

    let c = t.client.clone();
    let started = std::time::Instant::now();
    let pending = tokio::spawn(async move { resolve(&c, "r1", Duration::from_secs(20)).await });
    wait_until(|| !server.resolves().is_empty(), 5_000, "resolve on the wire").await;
    t.client.drop_connection();
    let result = pending.await.unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    let took = ms(started.elapsed());
    assert!((1_000..6_000).contains(&took), "took {took} ms");
    t.client.stop().await;
}

fn probing(server: &Server, probe_after: Duration, pong_deadline: Duration) -> common::mesh::TestClient {
    client_for(server, |c| {
        c.stall_probe_after = probe_after;
        c.stall_pong_deadline = pong_deadline;
    })
}

#[tokio::test]
async fn resolve_on_a_silent_connection_reconnects_and_resends() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 { Mode::Blackhole } else { Mode::Accept }
    })))
    .await;
    let t = probing(&server, Duration::from_millis(300), Duration::from_millis(300));
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
        if a == 1 { Mode::Blackhole } else { Mode::Accept }
    })))
    .await;
    let t = probing(&server, Duration::from_millis(300), Duration::from_millis(300));
    t.client.start();

    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    let out = t.out.text();
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
    let t = probing(&server, Duration::from_millis(300), Duration::from_millis(800));
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
        if a == 1 { Mode::Blackhole } else { Mode::Accept }
    })))
    .await;
    let t = probing(&server, Duration::from_millis(300), Duration::from_millis(300));
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
    assert_eq!(server.record(|r| r.pings.iter().filter(|c| **c == 1).count()), 1);
    t.client.stop().await;
}

#[tokio::test]
async fn deadline_ends_a_resolve_while_its_probe_waits() {
    let server = Server::start(Options::new(policy(|_| Mode::Blackhole))).await;
    let t = probing(&server, Duration::from_millis(300), Duration::from_secs(30));
    t.client.start();
    wait_until(|| t.client.is_connected(), 5_000, "connected").await;

    let started = std::time::Instant::now();
    let c = t.client.clone();
    let pending = tokio::spawn(async move { resolve(&c, "r1", Duration::from_millis(1_500)).await });
    wait_until(
        || server.record(|r| !r.pings.is_empty()),
        5_000,
        "stall probe ping",
    )
    .await;
    let result = pending.await.unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.reason.as_deref(), Some(fallback::CLIENT_DEADLINE_EXCEEDED));
    let took = ms(started.elapsed());
    assert!((1_400..2_500).contains(&took), "took {took} ms");
    t.client.stop().await;
}

#[tokio::test]
async fn backoff_grows_when_links_die_right_after_welcome() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&attempts);
    let server = Server::start(Options::new(policy(|_| Mode::WelcomeThenClose))).await;
    let t = client_for(&server, |c| {
        c.next_backoff_ms = Arc::new(move |a| {
            lock(&seen).push(a);
            50
        });
    });
    t.client.start();

    wait_until(
        || lock(&attempts).len() >= 4,
        15_000,
        "four backoffs after dropped links",
    )
    .await;

    assert_eq!(lock(&attempts)[..4], [1, 2, 3, 4]);
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_on_links_that_die_right_after_welcome_gives_up_after_one_resend() {
    let caps = [1_000u64, 2_000, 4_000, 8_000, 16_000, 30_000];
    let server = Server::start(Options::new(policy(|_| Mode::WelcomeThenClose)).silent_resolve()).await;
    let t = client_for(&server, |c| {
        c.next_backoff_ms = Arc::new(move |a| caps[usize::try_from(a).unwrap_or(6).clamp(1, 6) - 1]);
    });
    t.client.start();

    let started = std::time::Instant::now();
    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "took {:?}",
        started.elapsed()
    );
    let sends = server.resolves().iter().filter(|r| r.1 == "r1").count();
    assert!((1..=2).contains(&sends), "{sends}");
    t.client.stop().await;
}

#[tokio::test]
async fn link_that_stayed_up_resets_the_backoff() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&attempts);
    let mut opts = Options::new(policy(|a| if a <= 2 { Mode::Refuse } else { Mode::Accept }));
    opts.close_after = Arc::new(|a| (a == 3).then_some(Duration::from_millis(1_000)));
    let server = Server::start(opts).await;
    let t = client_for(&server, |c| {
        c.stable_link_after = Duration::from_millis(200);
        c.next_backoff_ms = Arc::new(move |a| {
            lock(&seen).push(a);
            50
        });
    });
    t.client.start();

    wait_until(
        || lock(&attempts).len() >= 3,
        15_000,
        "a backoff after the stable link dropped",
    )
    .await;

    assert_eq!(lock(&attempts)[..3], [1, 2, 1]);
    t.client.stop().await;
}

#[tokio::test]
async fn successful_connect_records_dns_and_tcp_timings() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept))).await;
    let t = client_for(&server, |_| {});
    t.client.start();

    wait_until(|| t.client.is_connected(), 10_000, "mesh connection").await;

    let probe = t.client.last_connect_probe();
    assert_eq!(probe.phase, Phase::Upgrade);
    assert!(probe.dns_ms.is_some());
    assert!(probe.tcp_ms.is_some());
    assert!(t.log().contains("[mesh] connected in "));
    assert!(
        t.log()
            .contains("[mesh][v3] negotiated subprotocol=<none> v3=false deflate-offered=false")
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

    let out = t.out.text();
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

#[tokio::test]
async fn only_the_first_failed_connect_reaches_the_console() {
    let server = Server::start(Options::new(policy(|a| {
        if a <= 2 { Mode::Refuse } else { Mode::Accept }
    })))
    .await;
    let t = client_for(&server, |c| c.next_backoff_ms = fixed_backoff(50));
    t.client.start();

    wait_until(
        || t.client.is_connected(),
        15_000,
        "mesh connection after two refused connects",
    )
    .await;

    let out = t.out.text();
    let failures: Vec<&str> = out.lines().filter(|l| l.contains("failed after")).collect();
    assert_eq!(failures.len(), 1, "{out}");
    assert!(failures[0].contains("during upgrade"));
    assert!(failures[0].contains("HandshakeError: server returned HTTP 503 instead of 101"));
    assert!(
        t.log()
            .contains("[mesh] reconnect attempt 1 failed: connect to vrcresolver.com failed after")
    );
    t.client.stop().await;
}

#[tokio::test]
async fn client_reconnects_after_the_first_connect_is_refused() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 { Mode::Refuse } else { Mode::Accept }
    })))
    .await;
    let t = client_for(&server, |_| {});
    t.client.start();

    wait_until(
        || t.client.is_connected(),
        10_000,
        "connection after a refused first connect",
    )
    .await;

    assert_eq!(server.attempts(), 2);
    assert_eq!(server.connections(), 1);
    t.client.stop().await;
}

#[tokio::test]
async fn client_keeps_retrying_until_the_mesh_comes_back() {
    let server = Server::start(Options::new(policy(|a| {
        if a <= 3 { Mode::Refuse } else { Mode::Accept }
    })))
    .await;
    let t = client_for(&server, |_| {});
    t.client.start();

    wait_until(
        || t.client.is_connected(),
        20_000,
        "connection after three refused connects",
    )
    .await;

    assert_eq!(server.attempts(), 4);
    t.client.stop().await;
}

#[tokio::test]
async fn every_connect_attempt_targets_the_public_mesh_host_as_the_watchdog() {
    let server = Server::start(Options::new(policy(|a| {
        if a <= 2 { Mode::Refuse } else { Mode::Accept }
    })))
    .await;
    let t = client_for(&server, |c| c.next_backoff_ms = fixed_backoff(50));
    t.client.start();

    wait_until(|| t.client.is_connected(), 15_000, "mesh connection").await;

    let hosts = lock(&t.hosts).clone();
    assert_eq!(hosts.len(), 3);
    assert!(hosts.iter().all(|h| h == PROXY_HOST));
    server.record(|r| {
        assert_eq!(r.paths.len(), 3);
        assert!(r.paths.iter().all(|p| p == "/mesh"));
        assert!(r.user_agents.iter().all(|ua| ua == "VRCResolver-Watchdog/test"));
    });
    t.client.stop().await;
}

#[tokio::test]
async fn resolve_asked_while_the_first_connect_fails_is_served_after_the_retry() {
    let server = Server::start(Options::new(policy(|a| {
        if a == 1 { Mode::Refuse } else { Mode::Accept }
    })))
    .await;
    let t = client_for(&server, |c| c.reconnect_grace = Duration::from_secs(10));
    t.client.start();

    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert_eq!(server.resolves().len(), 1);
    t.client.stop().await;
}

#[tokio::test]
async fn client_recovers_after_a_timed_out_first_connect_and_a_refusal() {
    let server = Server::start(Options::new(policy(|a| match a {
        1 => Mode::Hang,
        2 => Mode::Refuse,
        _ => Mode::Accept,
    })))
    .await;
    let t = client_for(&server, |c| {
        c.connect_deadline = Duration::from_millis(500);
        c.next_backoff_ms = fixed_backoff(50);
    });
    t.client.start();

    wait_until(
        || t.client.is_connected(),
        15_000,
        "connection after a hang and a refusal",
    )
    .await;

    assert_eq!(server.attempts(), 3);
    assert_eq!(server.connections(), 1);
    t.client.stop().await;
}

fn sent_deadline(json: &str) -> i64 {
    let v: Value = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    v["wrapper_deadline_ms"]
        .as_i64()
        .unwrap_or_else(|| panic!("no deadline in {json}"))
}

async fn connected(server: &Server) -> common::mesh::TestClient {
    let t = client_for(server, |_| {});
    t.client.start();
    wait_until(|| t.client.is_connected(), 5_000, "mesh connected").await;
    wait_until(|| t.client.server_protocol_version() > 0, 5_000, "welcome").await;
    t
}

#[tokio::test]
async fn resend_after_a_drop_carries_the_time_that_is_left() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).on_resolve(handler(
        |c: Conn, id: String| async move {
            if c.index == 1 {
                tokio::time::sleep(Duration::from_millis(400)).await;
                c.abort();
            } else {
                c.resolved(&id);
            }
        },
    )))
    .await;
    let t = connected(&server).await;

    let result = resolve(&t.client, "r1", Duration::from_secs(20)).await;

    assert_eq!(result.action, action::RESOLVED);
    let sent: Vec<i64> = server.record(|r| r.request_json.iter().map(|j| sent_deadline(j)).collect());
    assert_eq!(sent.len(), 2);
    assert!((18_000..=19_500).contains(&sent[0]), "{sent:?}");
    assert!(sent[1] <= sent[0] - 300, "{sent:?}");
    t.client.stop().await;
}

async fn ipc_round_trip(client: &MeshClient, line: &str) -> Value {
    let ipc = Ipc::builder(Arc::new(client.clone()), Ux::sink(), Log::disabled()).build();
    let (mut a, mut b) = tokio::io::duplex(1 << 20);
    a.write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    ipc.handle(&mut b).await;
    drop(b);
    let mut out = String::new();
    a.read_to_string(&mut out).await.unwrap_or_else(|e| panic!("{e}"));
    let first = out.lines().next().unwrap_or_else(|| panic!("no reply"));
    serde_json::from_str(first).unwrap_or_else(|e| panic!("{e}: {first}"))
}

fn request_line(wrapper_deadline_ms: Option<i32>) -> String {
    let extra = wrapper_deadline_ms.map_or_else(String::new, |ms| format!(",\"wrapper_deadline_ms\":{ms}"));
    format!(
        "{{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"avpro\"{extra}}}"
    )
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
    let sent: Vec<i64> = server.record(|r| r.request_json.iter().map(|j| sent_deadline(j)).collect());
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
    let sent: Vec<i64> = server.record(|r| r.request_json.iter().map(|j| sent_deadline(j)).collect());
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

fn v3_welcome(msgpack: bool) -> common::mesh::Welcome {
    Arc::new(move |_, hello: Option<&Value>| {
        let hello = hello.cloned().unwrap_or(Value::Null);
        let format = if msgpack { "msgpack" } else { "json" };
        if hello["welcome_hash"] == "hash1" {
            format!(
                r#"{{"action":"welcome_cached","protocol_version":3,"node":"node1","negotiated_format":"{format}"}}"#
            )
        } else {
            format!(
                r#"{{"action":"welcome","protocol_version":3,"node":"node1","server_version":"s1","features":["{}","{}","{}"],"welcome_hash":"hash1","negotiated_format":"{format}"}}"#,
                feature::PLAYBACK_FEEDBACK,
                feature::PLAYBACK_FEEDBACK_V2,
                feature::WELCOME_HOSTS
            )
        }
    })
}

#[tokio::test]
async fn v3_link_sends_client_hello_and_reuses_the_cached_welcome_on_reconnect() {
    let mut opts = Options::new(policy(|_| Mode::Accept));
    opts.v3 = true;
    opts.welcome = v3_welcome(false);
    let server = Server::start(opts).await;
    let t = client_for(&server, |c| c.next_backoff_ms = fixed_backoff(50));
    t.client.start();
    wait_until(|| t.client.server_protocol_version() == 3, 5_000, "v3 welcome").await;

    let first = resolve(&t.client, "r1", Duration::from_secs(10)).await;
    t.client.drop_connection();
    wait_until(|| server.connections() >= 2, 5_000, "second connection").await;
    let second = resolve(&t.client, "r2", Duration::from_secs(10)).await;

    assert_eq!(first.action, action::RESOLVED);
    assert_eq!(second.action, action::RESOLVED);
    let hellos: Vec<Value> = server.record(|r| {
        r.texts
            .iter()
            .filter(|(_, v)| v["action"] == action::CLIENT_HELLO)
            .map(|(_, v)| v.clone())
            .collect()
    });
    assert_eq!(hellos.len(), 2);
    assert!(hellos[0].get("welcome_hash").is_none());
    assert_eq!(hellos[1]["welcome_hash"], "hash1");
    assert_eq!(
        hellos[0]["accept_formats"],
        serde_json::json!(["msgpack", "json"])
    );
    assert_eq!(hellos[0]["client_id"].as_str().map(str::len), Some(32));
    assert_eq!(hellos[0]["client_id"], hellos[1]["client_id"]);
    assert!(t.log().contains("[mesh][v3] welcome_cached hit node=node1 v=3"));
    assert_eq!(t.client.server_version().as_deref(), Some("s1"));
    t.client.stop().await;
}

#[tokio::test]
async fn v3_client_id_survives_a_restart() {
    let mut opts = Options::new(policy(|_| Mode::Accept));
    opts.v3 = true;
    opts.welcome = v3_welcome(false);
    let server = Server::start(opts).await;
    let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().to_path_buf();
    let t = client_in(dir, &server, |_| {});
    let id = t.client.client_id().to_owned();
    let again =
        vrcr_watchdog::mesh::identity::load_or_create(&path.join(vrcr_watchdog::mesh::identity::FILE_NAME));
    assert_eq!(id, again);
}

#[tokio::test]
async fn msgpack_link_decodes_binary_answers() {
    let mut opts =
        Options::new(policy(|_| Mode::Accept)).on_resolve(handler(|c: Conn, id: String| async move {
            let mut out = Vec::new();
            let frame = rmpv::Value::Array(vec![
                rmpv::Value::from(action::RESOLVED),
                rmpv::Value::from(id.as_str()),
                rmpv::Value::from(MEDIA),
            ]);
            rmpv::encode::write_value(&mut out, &frame).unwrap_or_else(|e| panic!("{e}"));
            c.binary(out);
        }));
    opts.v3 = true;
    opts.welcome = v3_welcome(true);
    let server = Server::start(opts).await;
    let t = client_for(&server, |_| {});
    t.client.start();
    wait_until(
        || t.client.negotiated_format() == "msgpack",
        5_000,
        "msgpack welcome",
    )
    .await;

    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    let frame: Value = serde_json::from_slice(&result.frame).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(frame["url"], MEDIA);
    assert_eq!(frame["id"], "r1");
    t.client.stop().await;
}

#[tokio::test]
async fn binary_frame_on_a_json_link_drops_the_connection() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).on_resolve(handler(
        |c: Conn, id: String| async move {
            if c.index == 1 {
                c.binary(vec![0x91, 0xA4, b'p', b'o', b'n', b'g']);
            } else {
                c.resolved(&id);
            }
        },
    )))
    .await;
    let t = client_for(&server, |c| c.next_backoff_ms = fixed_backoff(50));
    t.client.start();

    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert_eq!(server.connections(), 2);
    assert!(
        t.out
            .text()
            .contains("unexpected Binary frame on json-negotiated connection")
    );
    t.client.stop().await;
}

#[tokio::test]
async fn playback_feedback_honours_the_advertised_features() {
    let mut opts = Options::new(policy(|_| Mode::Accept));
    opts.welcome = Arc::new(|_, _| {
        format!(
            r#"{{"action":"welcome","protocol_version":3,"node":"node1","features":["{}"]}}"#,
            feature::PLAYBACK_FEEDBACK
        )
    });
    let server = Server::start(opts).await;
    let t = connected(&server).await;
    let _ = resolve(&t.client, "r1", Duration::from_secs(10)).await;
    let fb = |kind: &'static str| Feedback {
        url: MEDIA.to_owned(),
        kind,
        ms_since_open: 1_200,
        delivered_height: Some(720),
        detail: Some("ignored".to_owned()),
        correlation_id: None,
    };

    t.client.send_playback_feedback(fb(feedback::CACHE_PLAY));
    t.client.send_playback_feedback(fb(feedback::PLAYING));
    wait_until(
        || {
            server.record(|r| {
                r.texts
                    .iter()
                    .any(|(_, v)| v["action"] == action::PLAYBACK_FEEDBACK)
            })
        },
        5_000,
        "feedback frame",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let frames: Vec<Value> = server.record(|r| {
        r.texts
            .iter()
            .filter(|(_, v)| v["action"] == action::PLAYBACK_FEEDBACK)
            .map(|(_, v)| v.clone())
            .collect()
    });
    assert_eq!(frames.len(), 1, "{frames:?}");
    let f = &frames[0];
    assert_eq!(f["kind"], feedback::PLAYING);
    assert_eq!(f["delivered_height"], 720);
    assert_eq!(f["ms_since_open"], 1_200);
    assert_eq!(f["correlation_id"], "r1");
    assert!(f.get("detail").is_none());
    assert_eq!(f["client_id"].as_str(), Some(t.client.client_id()));
    t.client.stop().await;
}

#[tokio::test]
async fn stop_fails_waiting_resolves_and_closes_the_link() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).silent_resolve()).await;
    let t = connected(&server).await;
    let c = t.client.clone();
    let pending = tokio::spawn(async move { resolve(&c, "r1", Duration::from_secs(20)).await });
    wait_until(|| !server.resolves().is_empty(), 5_000, "resolve on the wire").await;

    t.client.stop().await;
    let result = pending.await.unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    assert!(
        t.out
            .text()
            .contains("failing 1 pending requests reason=server_unreachable ids=r1")
    );
    assert!(!t.client.is_connected());
}
