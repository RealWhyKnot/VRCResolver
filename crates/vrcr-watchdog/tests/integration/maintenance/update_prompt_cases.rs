use std::cell::RefCell;
use std::process::Command;

use vrcr_watchdog::maintenance::update::{Prompter, UpdateTicket, check_and_offer};
use vrcr_watchdog::maintenance::update_dialog::UpdateDecision;
use vrcr_watchdog::settings::Store;

use super::*;

const CURRENT: &str = "v2026.11.20.0";

thread_local! {
    static LAUNCHES: RefCell<Vec<(Command, UpdateTicket)>> = const { RefCell::new(Vec::new()) };
}

fn record_launch(cmd: Command, ticket: UpdateTicket) -> std::io::Result<()> {
    LAUNCHES.with(|launches| launches.borrow_mut().push((cmd, ticket)));
    Ok(())
}

fn launch_count() -> usize {
    LAUNCHES.with(|launches| launches.borrow().len())
}

fn release(tag: &str) -> Release {
    Release {
        tag: tag.to_owned(),
        html_url: String::new(),
        prerelease: false,
    }
}

type Calls = Arc<Mutex<Vec<(String, String)>>>;

struct Setup {
    dir: TempDir,
    out: Capture,
    calls: Calls,
    prompter: Prompter,
}

fn setup(decision: UpdateDecision, interactive: bool) -> Setup {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join(update::UPDATER_EXE), b"x").unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let prompter = Prompter {
        settings: Arc::new(open_store(&dir)),
        gate: UpdateGate::default(),
        install_dir: dir.path().to_owned(),
        spawn: record_launch,
        current: CURRENT.to_owned(),
        interactive,
        closing: CancellationToken::new(),
        ux,
        prompt: Arc::new(move |version, current, _closing| {
            lock(&seen).push((version.to_owned(), current.to_owned()));
            decision
        }),
    };
    Setup {
        dir,
        out,
        calls,
        prompter,
    }
}

fn open_store(dir: &TempDir) -> Store {
    Store::new(dir.path().join("settings.json"), Box::new(|_| {}))
}

#[test]
fn skipped_version_suppresses_the_prompt() {
    let run = setup(UpdateDecision::UpdateNow, true);
    run.prompter
        .settings
        .set_skipped_version(Some("2026.11.24.0"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(run.prompter.offer(&release("v2026.11.24.0")), None);
    assert!(lock(&run.calls).is_empty());
    assert_eq!(launch_count(), 0);
    assert!(
        run.out
            .text()
            .contains("You skipped version 2026.11.24.0. Type update to install it.")
    );
}

#[test]
fn newer_release_after_a_skipped_one_prompts_again() {
    let run = setup(UpdateDecision::Later, true);
    run.prompter
        .settings
        .set_skipped_version(Some("v2026.11.24.0"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        run.prompter.offer(&release("v2026.12.1.0")),
        Some(UpdateDecision::Later)
    );
    assert_eq!(
        lock(&run.calls).clone(),
        vec![("2026.12.1.0".to_owned(), "2026.11.20.0".to_owned())]
    );
    assert!(run.out.text().contains("Version 2026.12.1.0 is available."));
}

#[test]
fn update_now_starts_one_updater_even_when_asked_twice() {
    let run = setup(UpdateDecision::UpdateNow, true);
    assert_eq!(
        run.prompter.offer(&release("v2026.11.24.0")),
        Some(UpdateDecision::UpdateNow)
    );
    assert_eq!(launch_count(), 1);
    LAUNCHES.with(|launches| {
        let launches = launches.borrow();
        let (cmd, _) = &launches[0];
        assert_eq!(
            std::path::PathBuf::from(cmd.get_program()),
            run.dir.path().join(update::UPDATER_EXE)
        );
        assert!(cmd.get_envs().any(|(key, value)| {
            key == update::UPDATE_REQUESTED_ENV && value == Some(std::ffi::OsStr::new("1"))
        }));
    });
    assert!(run.prompter.gate.is_running());
    assert_eq!(
        run.prompter
            .gate
            .start(run.dir.path(), |_, _| panic!("must not spawn twice")),
        UpdateStart::AlreadyRunning
    );
    assert_eq!(
        run.prompter.offer(&release("v2026.11.24.0")),
        Some(UpdateDecision::UpdateNow)
    );
    assert_eq!(launch_count(), 1);
    let console = run.out.text();
    assert!(console.contains("Starting the update to 2026.11.24.0."));
    assert!(console.contains("An update is already running."));
    LAUNCHES.with(|launches| launches.borrow_mut().clear());
    assert!(!run.prompter.gate.is_running());
}

#[test]
fn skip_version_persists_the_choice() {
    let run = setup(UpdateDecision::SkipVersion, true);
    assert_eq!(
        run.prompter.offer(&release("v2026.11.24.0")),
        Some(UpdateDecision::SkipVersion)
    );
    assert_eq!(launch_count(), 0);
    assert_eq!(
        open_store(&run.dir).skipped_version().as_deref(),
        Some("2026.11.24.0")
    );
    assert!(
        run.out
            .text()
            .contains("You skipped version 2026.11.24.0. Type update to install it.")
    );
    assert_eq!(run.prompter.offer(&release("v2026.11.24.0")), None);
    assert_eq!(lock(&run.calls).len(), 1);
}

#[test]
fn later_does_not_persist_a_skip() {
    let run = setup(UpdateDecision::Later, true);
    assert_eq!(
        run.prompter.offer(&release("v2026.11.24.0")),
        Some(UpdateDecision::Later)
    );
    assert_eq!(launch_count(), 0);
    assert_eq!(open_store(&run.dir).skipped_version(), None);
    assert!(!run.dir.path().join("settings.json").exists());
    assert!(
        run.out
            .text()
            .contains("Update postponed. Type update anytime to install it.")
    );
}

#[test]
fn non_interactive_runs_announce_without_a_prompt() {
    let run = setup(UpdateDecision::UpdateNow, false);
    assert_eq!(run.prompter.offer(&release("v2026.11.24.0")), None);
    assert!(lock(&run.calls).is_empty());
    assert_eq!(launch_count(), 0);
    assert!(
        run.out
            .text()
            .contains("Version 2026.11.24.0 is available. Type update to install it.")
    );
}

#[test]
fn missing_updater_announces_without_a_prompt() {
    let run = setup(UpdateDecision::UpdateNow, true);
    fs::remove_file(run.dir.path().join(update::UPDATER_EXE))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.prompter.offer(&release("v2026.11.24.0")), None);
    assert!(lock(&run.calls).is_empty());
    assert_eq!(launch_count(), 0);
    let text = run.out.text();
    assert!(text.contains("Version 2026.11.24.0 is available. Type update to install it."));
}

#[test]
fn closing_app_skips_the_prompt() {
    let run = setup(UpdateDecision::UpdateNow, true);
    run.prompter.closing.cancel();
    assert_eq!(run.prompter.offer(&release("v2026.11.24.0")), None);
    assert!(lock(&run.calls).is_empty());
    assert_eq!(launch_count(), 0);
}

#[test]
fn background_check_prompts_only_for_newer_releases() {
    let run = setup(UpdateDecision::Later, true);
    let newer = json!({"tag_name": "v2026.11.24.0", "html_url": "", "prerelease": false});
    block_on(check_and_offer(false, run.prompter.clone(), |url| {
        assert_eq!(url, update::STABLE_LATEST_URL);
        async move { Some(newer.to_string()) }
    }));
    assert_eq!(
        lock(&run.calls).clone(),
        vec![("2026.11.24.0".to_owned(), "2026.11.20.0".to_owned())]
    );
    let same = json!({"tag_name": CURRENT, "html_url": "", "prerelease": false});
    block_on(check_and_offer(
        false,
        run.prompter.clone(),
        |_| async move { Some(same.to_string()) },
    ));
    assert_eq!(lock(&run.calls).len(), 1);
}
