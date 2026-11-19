use std::fs;

use tempfile::TempDir;

use super::*;
use crate::terminal::commands::Registry;

#[test]
fn window_close_restores_ytdlp_and_marks_a_clean_exit_without_quit() {
    let commands = Registry::with_defaults();
    assert!(commands.get("quit").is_none());
    assert!(commands.get("exit").is_none());
    check_window_close(false);
}

#[test]
fn window_close_restores_ytdlp_and_ends_a_waiting_permission_dialog() {
    check_window_close(true);
}

fn check_window_close(waiting_dialog: bool) {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let state_dir = dir.path().join("state");
    let tools_dir = dir.path().join("Tools");
    fs::create_dir(&tools_dir).unwrap_or_else(|e| panic!("{e}"));
    let target = tools_dir.join(tools::TARGET_NAME);
    let original = tools_dir.join(tools::OG_NAME);
    fs::write(&target, vrcr_shared::wrapper_identity::marker()).unwrap_or_else(|e| panic!("{e}"));
    fs::write(&original, b"original video player").unwrap_or_else(|e| panic!("{e}"));
    let patcher = PatchManager::new(
        PatchConfig {
            install_dir: dir.path().to_path_buf(),
            state_dir: state_dir.clone(),
            vrc_tools_dir: Some(tools_dir),
            tick_delay: patch::DEFAULT_TICK_DELAY,
        },
        Ux::sink(),
        Log::disabled(),
    );
    let runtime = Runtime::new().unwrap_or_else(|e| panic!("{e}"));
    let life = Lifecycle::new(runtime.handle().clone());
    let slow_steps = Arc::new(AtomicBool::new(false));
    let steps = Arc::clone(&slow_steps);
    let coordinator = ShutdownCoordinator::new(
        Arc::new(move |reason| Ok(patcher.restore_now(reason))),
        Arc::new(move || {
            steps.store(true, Ordering::Relaxed);
            Vec::new()
        }),
        Arc::new(|_| {}),
    );
    assert!(life.coordinator.set(coordinator).is_ok());
    let dialog = waiting_dialog.then(|| {
        let (entered, waiting) = std::sync::mpsc::channel();
        let closing = life.closing.clone();
        let worker = runtime.spawn_blocking(move || {
            let ux = Ux::sink();
            let hosts = HostsManager::new(
                Box::new(|| Ok(false)),
                Box::new(|_| panic!("closing setup must skip elevation")),
                ux.clone(),
            );
            setup::run(
                &hosts,
                false,
                &closing,
                &|changes| {
                    assert_eq!(changes, setup::Changes::Hosts);
                    entered.send(()).expect("test is waiting for the dialog");
                    Handle::current()
                        .block_on(async {
                            tokio::time::timeout(CLOSE_WAIT, closing.cancelled()).await
                        })
                        .expect("closing the window cancels the dialog");
                    setup::Decision::Closed
                },
                &ux,
                |_| {},
            )
        });
        waiting
            .recv_timeout(CLOSE_WAIT)
            .expect("permission dialog is waiting before close");
        worker
    });
    let started = std::time::Instant::now();
    std::thread::spawn(move || life.on_ctrl(CtrlEvent::Close))
        .join()
        .expect("Windows close callback completed");
    if let Some(dialog) = dialog {
        let outcome = runtime.block_on(async {
            tokio::time::timeout_at((started + CLOSE_WAIT).into(), dialog)
                .await
                .expect("permission dialog ended within the close budget")
                .expect("permission worker completed")
        });
        assert_eq!(outcome.1, Err(crate::maintenance::SetupError::Declined));
    }
    assert!(!slow_steps.load(Ordering::Relaxed));
    assert_eq!(
        fs::read(&target).unwrap_or_default(),
        b"original video player"
    );
    assert!(!original.exists());
    assert!(state_dir.join("clean_exit.flag").is_file());
    assert!(started.elapsed() < CLOSE_WAIT);
}
