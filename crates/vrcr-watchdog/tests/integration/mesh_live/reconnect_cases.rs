use super::*;

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
    let mut opts = Options::new(policy(|a| {
        if a == 1 {
            Mode::Refuse
        } else {
            Mode::SlowAccept
        }
    }));
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
    let mut opts = Options::new(policy(|a| {
        if a == 1 {
            Mode::SlowAccept
        } else {
            Mode::Accept
        }
    }));
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
    wait_until(
        || server.attempts() >= 2,
        6_000,
        "reconnect after the backoff",
    )
    .await;
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
        t.out.logs(),
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
    let server =
        Arc::new(Server::start(Options::new(policy(|_| Mode::Accept)).silent_resolve()).await);
    let t = client_for(&server, |c| {
        c.reconnect_grace = Duration::from_millis(1_500)
    });
    t.client.start();
    wait_until(|| t.client.is_connected(), 5_000, "connected").await;
    server.stop_accepting();

    let c = t.client.clone();
    let started = std::time::Instant::now();
    let pending = tokio::spawn(async move { resolve(&c, "r1", Duration::from_secs(20)).await });
    wait_until(
        || !server.resolves().is_empty(),
        5_000,
        "resolve on the wire",
    )
    .await;
    t.client.drop_connection();
    let result = pending.await.unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result.action, action::FALLBACK_NATIVE);
    assert_eq!(result.reason.as_deref(), Some(fallback::SERVER_UNREACHABLE));
    let took = ms(started.elapsed());
    assert!((1_000..6_000).contains(&took), "took {took} ms");
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
    let server =
        Server::start(Options::new(policy(|_| Mode::WelcomeThenClose)).silent_resolve()).await;
    let t = client_for(&server, |c| {
        c.next_backoff_ms =
            Arc::new(move |a| caps[usize::try_from(a).unwrap_or(6).clamp(1, 6) - 1]);
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

    let out = t.out.logs();
    let failures: Vec<&str> = out.lines().filter(|l| l.contains("failed after")).collect();
    assert_eq!(failures.len(), 1, "{out}");
    assert!(failures[0].contains("during upgrade"));
    assert!(failures[0].contains("http_503"));
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
        assert!(
            r.user_agents
                .iter()
                .all(|ua| ua == "VRCResolver-Watchdog/test")
        );
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
    let sent: Vec<i64> =
        server.record(|r| r.request_json.iter().map(|j| sent_deadline(j)).collect());
    assert_eq!(sent.len(), 2);
    assert!((18_000..=19_500).contains(&sent[0]), "{sent:?}");
    assert!(sent[1] <= sent[0] - 300, "{sent:?}");
    t.client.stop().await;
}
