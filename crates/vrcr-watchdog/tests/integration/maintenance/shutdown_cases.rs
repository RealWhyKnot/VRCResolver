use super::*;

#[test]
fn second_caller_shares_the_run_and_waits_for_the_restore() {
    let rig = ShutdownRig::new();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let wait = std::sync::Arc::clone(&gate);
    let c = rig.coordinator(
        move |_| {
            let (m, cv) = &*wait;
            let mut open = lock(m);
            while !*open {
                open = cv.wait(open).unwrap_or_else(|e| e.into_inner());
            }
            drop(open);
            Ok(true)
        },
        Vec::new,
    );
    rt().block_on(async {
        let first = c.run("ctrl-c", false);
        let second = c.run("quit", false);
        let second = tokio::spawn(second);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!second.is_finished());
        {
            let (m, cv) = &*gate;
            *lock(m) = true;
            cv.notify_all();
        }
        tokio::time::timeout(GENEROUS, second)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        tokio::time::timeout(GENEROUS, first)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    let lines = rig.lines();
    assert!(lines.contains(&"shutdown trigger=ctrl-c".to_owned()));
    assert!(!lines.iter().any(|l| l.contains("quit")));
}

#[test]
fn restore_runs_before_slow_steps() {
    let rig = ShutdownRig::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let started = Instant::now();
    let restored_at = Arc::new(Mutex::new(None));
    let (o1, o2, at) = (
        std::sync::Arc::clone(&order),
        std::sync::Arc::clone(&order),
        std::sync::Arc::clone(&restored_at),
    );
    let c = rig.coordinator(
        move |_| {
            *lock(&at) = Some(started.elapsed());
            lock(&o1).push("restore");
            Ok(true)
        },
        move || {
            let o = std::sync::Arc::clone(&o2);
            vec![ShutdownStep::new(
                "slow",
                Duration::from_secs(10),
                move || async move {
                    lock(&o).push("slow");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok(())
                },
            )]
        },
    );
    rt().block_on(async {
        let run = c.run("quit", false);
        drop(tokio::spawn(run));
        let deadline = Instant::now() + GENEROUS;
        while lock(&restored_at).is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let at = lock(&restored_at).unwrap_or_else(|| panic!("restore never ran"));
    assert!(at < Duration::from_secs(1), "{at:?}");
    assert_eq!(lock(&order).first().copied(), Some("restore"));
}

#[test]
fn fast_shutdown_restores_and_skips_steps() {
    let rig = ShutdownRig::new();
    let restored = Arc::new(AtomicBool::new(false));
    let ran = Arc::new(AtomicBool::new(false));
    let (r, s) = (
        std::sync::Arc::clone(&restored),
        std::sync::Arc::clone(&ran),
    );
    let c = rig.coordinator(
        move |_| {
            r.store(true, Ordering::SeqCst);
            Ok(true)
        },
        move || {
            let s = std::sync::Arc::clone(&s);
            vec![ShutdownStep::new(
                "terminal",
                Duration::from_secs(1),
                move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )]
        },
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("console-close", true))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(restored.load(Ordering::SeqCst));
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(
        rig.lines(),
        vec!["shutdown trigger=console-close", "tools restored=true"]
    );
}

#[test]
fn restore_failure_is_logged_and_steps_still_run() {
    for restore_errors in [false, true] {
        let rig = ShutdownRig::new();
        let ran = Arc::new(AtomicBool::new(false));
        let s = std::sync::Arc::clone(&ran);
        let c = rig.coordinator(
            move |_| {
                if restore_errors {
                    Err("PermissionDenied: tools locked".to_owned())
                } else {
                    Ok(false)
                }
            },
            move || {
                let s = std::sync::Arc::clone(&s);
                vec![ShutdownStep::new(
                    "ipc",
                    Duration::from_secs(1),
                    move || async move {
                        s.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )]
            },
        );
        rt().block_on(async {
            tokio::time::timeout(GENEROUS, c.run("quit", false))
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        });
        assert!(ran.load(Ordering::SeqCst));
        let lines = rig.lines();
        assert!(lines.contains(&"tools restored=false".to_owned()));
        assert_eq!(
            lines
                .iter()
                .any(|l| l.contains("tools restore threw") && l.contains("tools locked")),
            restore_errors
        );
    }
}

#[test]
fn restore_panic_is_logged_and_steps_still_run() {
    let rig = ShutdownRig::new();
    let ran = Arc::new(AtomicBool::new(false));
    let s = std::sync::Arc::clone(&ran);
    let c = rig.coordinator(
        |_| panic!("boom"),
        move || {
            let s = std::sync::Arc::clone(&s);
            vec![ShutdownStep::new(
                "ipc",
                Duration::from_secs(1),
                move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )]
        },
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(ran.load(Ordering::SeqCst));
    assert!(
        rig.lines()
            .contains(&"tools restore threw panicked".to_owned())
    );
}

#[test]
fn step_over_budget_does_not_block_later_steps() {
    let rig = ShutdownRig::new();
    let later = Arc::new(AtomicBool::new(false));
    let s = std::sync::Arc::clone(&later);
    let c = rig.coordinator(
        |_| Ok(true),
        move || {
            let s = std::sync::Arc::clone(&s);
            vec![
                ShutdownStep::new("mesh", Duration::from_millis(100), std::future::pending),
                ShutdownStep::new(
                    "resolve-cache",
                    Duration::from_secs(1),
                    move || async move {
                        s.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                ),
            ]
        },
    );
    let clock = Instant::now();
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(later.load(Ordering::SeqCst));
    assert!(clock.elapsed() < Duration::from_secs(2));
    assert!(
        rig.lines()
            .contains(&"shutdown step mesh took longer than 100 ms; moving on".to_owned()),
        "{:?}",
        rig.lines()
    );
}

#[test]
fn failing_or_panicking_steps_do_not_block_later_steps() {
    let rig = ShutdownRig::new();
    let later = Arc::new(AtomicBool::new(false));
    let s = std::sync::Arc::clone(&later);
    let c = rig.coordinator(
        |_| Ok(true),
        move || {
            let s = std::sync::Arc::clone(&s);
            vec![
                ShutdownStep::new("relay", Duration::from_secs(1), || async {
                    Err("listener gone".to_owned())
                }),
                ShutdownStep::new("heartbeat", Duration::from_secs(1), || async {
                    panic!("bad")
                }),
                ShutdownStep::new("mesh", Duration::from_secs(1), move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                }),
            ]
        },
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(later.load(Ordering::SeqCst));
    let lines = rig.lines();
    assert!(lines.contains(&"shutdown step relay failed: listener gone".to_owned()));
    assert!(lines.contains(&"shutdown step heartbeat failed: panicked".to_owned()));
}

#[test]
fn spent_budget_skips_the_remaining_steps() {
    let rig = ShutdownRig::new();
    let ran = Arc::new(AtomicBool::new(false));
    let s = std::sync::Arc::clone(&ran);
    let c = rig
        .coordinator(
            |_| {
                std::thread::sleep(Duration::from_millis(80));
                Ok(true)
            },
            move || {
                let s = std::sync::Arc::clone(&s);
                vec![ShutdownStep::new(
                    "ipc",
                    Duration::from_secs(1),
                    move || async move {
                        s.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )]
            },
        )
        .with_total_budget(Duration::from_millis(50));
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(!ran.load(Ordering::SeqCst));
    assert!(
        rig.lines()
            .contains(&"shutdown budget spent; skipping ipc and the rest".to_owned())
    );
}

#[test]
fn shutdown_before_steps_exist_still_restores_with_the_trigger() {
    let rig = ShutdownRig::new();
    let reason = Arc::new(Mutex::new(String::new()));
    let r = std::sync::Arc::clone(&reason);
    let c = rig.coordinator(
        move |why| {
            *lock(&r) = why.to_owned();
            Ok(true)
        },
        Vec::new,
    );
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("ctrl-c", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert_eq!(*lock(&reason), "ctrl-c");
    assert_eq!(
        rig.lines(),
        vec!["shutdown trigger=ctrl-c", "tools restored=true"]
    );
}

#[test]
fn steps_are_read_when_the_shutdown_starts() {
    let rig = ShutdownRig::new();
    let registered = Arc::new(AtomicBool::new(false));
    let ran = Arc::new(AtomicBool::new(false));
    let (reg, s) = (
        std::sync::Arc::clone(&registered),
        std::sync::Arc::clone(&ran),
    );
    let c = rig.coordinator(
        |_| Ok(true),
        move || {
            if !reg.load(Ordering::SeqCst) {
                return Vec::new();
            }
            let s = std::sync::Arc::clone(&s);
            vec![ShutdownStep::new(
                "heartbeat",
                Duration::from_secs(1),
                move || async move {
                    s.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )]
        },
    );
    registered.store(true, Ordering::SeqCst);
    assert!(!c.started());
    rt().block_on(async {
        tokio::time::timeout(GENEROUS, c.run("quit", false))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    assert!(c.started());
    assert!(ran.load(Ordering::SeqCst));
}

#[test]
fn cancel_all_stops_every_task_even_when_one_panicked() {
    let stopped = Arc::new(AtomicBool::new(false));
    let second = CancellationToken::new();
    let result = rt().block_on(async {
        let first = CancellationToken::new();
        let a = first.clone();
        let failing = tokio::spawn(async move {
            a.cancelled().await;
            panic!("boom");
        });
        let (b, seen) = (second.clone(), std::sync::Arc::clone(&stopped));
        let quiet = tokio::spawn(async move {
            b.cancelled().await;
            seen.store(true, Ordering::SeqCst);
        });
        tokio::time::timeout(
            GENEROUS,
            cancel_all(vec![(first, failing), (second.clone(), quiet)]),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"))
    });
    assert_eq!(result, Err("panicked".to_owned()));
    assert!(second.is_cancelled());
    assert!(stopped.load(Ordering::SeqCst));
}
