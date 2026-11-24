use std::cell::RefCell;
use std::process::Command;

use vrcr_watchdog::maintenance::update::{
    CheckFlight, CheckStart, Prompter, UpdateTicket, check_and_offer, check_and_offer_manual,
    normalize_version, start_manual_check,
};
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
        body: None,
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
        flight: CheckFlight::default(),
        install_dir: dir.path().to_owned(),
        spawn: record_launch,
        current: CURRENT.to_owned(),
        interactive,
        closing: CancellationToken::new(),
        ux,
        prompt: Arc::new(move |release, current, _closing| {
            lock(&seen).push((
                normalize_version(&release.tag).to_owned(),
                current.to_owned(),
            ));
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
    assert!(run.out.text().contains(
        "Version 2026.11.24.0 is available, but you skipped it. Type update to review it."
    ));
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
            .contains("You skipped version 2026.11.24.0. Type update to review it.")
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

fn fetch_of(
    body: Option<serde_json::Value>,
) -> impl FnOnce(&'static str) -> std::future::Ready<Option<String>> {
    move |_| std::future::ready(body.map(|doc| doc.to_string()))
}

fn newer() -> serde_json::Value {
    json!({"tag_name": "v2026.11.24.0", "html_url": "", "prerelease": false})
}

fn manual(run: &Setup, body: Option<serde_json::Value>) {
    let ticket = run
        .prompter
        .flight
        .try_enter()
        .unwrap_or_else(|| panic!("flight is free"));
    block_on(check_and_offer_manual(
        false,
        run.prompter.clone(),
        ticket,
        fetch_of(body),
    ));
}

#[test]
fn manual_check_clears_a_skip_and_prompts_for_that_version() {
    let run = setup(UpdateDecision::Later, true);
    run.prompter
        .settings
        .set_skipped_version(Some("2026.11.24.0"))
        .unwrap_or_else(|e| panic!("{e}"));
    manual(&run, Some(newer()));
    assert_eq!(
        lock(&run.calls).clone(),
        vec![("2026.11.24.0".to_owned(), "2026.11.20.0".to_owned())]
    );
    assert_eq!(open_store(&run.dir).skipped_version(), None);
}

#[test]
fn manual_check_can_skip_a_version_and_still_reprompt() {
    let run = setup(UpdateDecision::SkipVersion, true);
    manual(&run, Some(newer()));
    assert_eq!(
        open_store(&run.dir).skipped_version().as_deref(),
        Some("2026.11.24.0")
    );
    manual(&run, Some(newer()));
    assert_eq!(lock(&run.calls).len(), 2);
    assert_eq!(
        open_store(&run.dir).skipped_version().as_deref(),
        Some("2026.11.24.0")
    );
}

#[test]
fn manual_check_reports_up_to_date() {
    let run = setup(UpdateDecision::UpdateNow, true);
    manual(&run, Some(json!({"tag_name": CURRENT, "html_url": ""})));
    assert!(
        run.out
            .text()
            .contains("You're already on the latest version (2026.11.20.0).")
    );
    assert!(lock(&run.calls).is_empty());
}

#[test]
fn manual_failure_is_reported_and_keeps_the_saved_skip() {
    let run = setup(UpdateDecision::UpdateNow, true);
    run.prompter
        .settings
        .set_skipped_version(Some("2026.11.24.0"))
        .unwrap_or_else(|e| panic!("{e}"));
    manual(&run, None);
    assert!(
        run.out
            .text()
            .contains("Couldn't check for updates. Try again in a moment.")
    );
    assert_eq!(
        open_store(&run.dir).skipped_version().as_deref(),
        Some("2026.11.24.0")
    );
    assert!(lock(&run.calls).is_empty());
}

#[test]
fn background_check_stays_quiet_when_up_to_date_or_failed() {
    let run = setup(UpdateDecision::UpdateNow, true);
    block_on(check_and_offer(
        false,
        run.prompter.clone(),
        fetch_of(Some(json!({"tag_name": CURRENT, "html_url": ""}))),
    ));
    block_on(check_and_offer(false, run.prompter.clone(), fetch_of(None)));
    assert_eq!(run.out.text(), "");
    assert!(lock(&run.calls).is_empty());
}

#[test]
fn flight_allows_one_check_at_a_time_and_frees_on_drop() {
    let flight = CheckFlight::default();
    let first = flight.try_enter();
    assert!(first.is_some());
    assert!(flight.try_enter().is_none());
    drop(first);
    assert!(flight.try_enter().is_some());
}

#[test]
fn background_check_is_skipped_while_another_check_is_open() {
    let run = setup(UpdateDecision::Later, true);
    let held = run.prompter.flight.try_enter();
    block_on(check_and_offer(
        false,
        run.prompter.clone(),
        fetch_of(Some(newer())),
    ));
    assert!(lock(&run.calls).is_empty());
    drop(held);
    block_on(check_and_offer(
        false,
        run.prompter.clone(),
        fetch_of(Some(newer())),
    ));
    assert_eq!(lock(&run.calls).len(), 1);
}

#[test]
fn manual_start_refuses_while_busy_or_installing() {
    let run = setup(UpdateDecision::Later, true);
    let runtime = rt();
    let held = run.prompter.flight.try_enter();
    assert_eq!(
        start_manual_check(&run.prompter, runtime.handle()),
        CheckStart::Checking
    );
    if let Some(ticket) = &held {
        ticket.mark_prompting();
    }
    assert_eq!(
        start_manual_check(&run.prompter, runtime.handle()),
        CheckStart::Prompting
    );
    drop(held);
    let started = run.prompter.gate.start(run.dir.path(), record_launch);
    assert_eq!(started, UpdateStart::Started);
    assert_eq!(
        start_manual_check(&run.prompter, runtime.handle()),
        CheckStart::Installing
    );
    LAUNCHES.with(|launches| launches.borrow_mut().clear());
}

#[test]
fn failed_checks_log_the_reason_for_startup_and_manual() {
    let run = setup(UpdateDecision::UpdateNow, true);
    block_on(check_and_offer(false, run.prompter.clone(), fetch_of(None)));
    assert_eq!(run.out.text(), "");
    assert!(
        run.out
            .logs()
            .contains("update check failed: GitHub could not be reached")
    );
    manual(&run, None);
    assert_eq!(
        run.out
            .logs()
            .matches("update check failed: GitHub could not be reached")
            .count(),
        2
    );
}

#[test]
fn manual_check_without_a_dialog_starts_the_update() {
    let run = setup(UpdateDecision::Later, false);
    manual(&run, Some(newer()));
    assert!(lock(&run.calls).is_empty());
    assert!(
        run.out
            .text()
            .contains("Starting the update to 2026.11.24.0.")
    );
    assert!(!run.out.text().contains("Type update to install it."));
}

#[test]
fn manual_check_explains_a_missing_updater() {
    let run = setup(UpdateDecision::UpdateNow, true);
    fs::remove_file(run.dir.path().join(update::UPDATER_EXE))
        .unwrap_or_else(|error| panic!("{error}"));
    manual(&run, Some(newer()));
    assert!(lock(&run.calls).is_empty());
    assert_eq!(launch_count(), 0);
    let text = run.out.text();
    assert!(text.contains("Version 2026.11.24.0 is available, but the updater is missing."));
    assert!(!text.contains("Type update to install it."));
}

#[test]
fn flight_reports_the_open_window_while_the_dialog_is_up() {
    let run = setup(UpdateDecision::Later, true);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let watcher = Arc::clone(&seen);
    let flight = run.prompter.flight.clone();
    let mut prompter = run.prompter.clone();
    prompter.prompt = Arc::new(move |_, _, _| {
        lock(&watcher).push((flight.try_enter().is_none(), flight.busy_reason()));
        UpdateDecision::Later
    });
    block_on(check_and_offer(false, prompter, fetch_of(Some(newer()))));
    assert_eq!(*lock(&seen), vec![(true, CheckStart::Prompting)]);
    assert!(run.prompter.flight.try_enter().is_some());
}

#[test]
fn flight_is_freed_when_the_dialog_panics() {
    let run = setup(UpdateDecision::Later, true);
    let mut prompter = run.prompter.clone();
    prompter.prompt = Arc::new(|_, _, _| panic!("dialog failed"));
    block_on(check_and_offer(false, prompter, fetch_of(Some(newer()))));
    assert!(run.prompter.flight.try_enter().is_some());
    assert!(
        run.out
            .logs()
            .contains("update prompt stopped unexpectedly")
    );
}

#[test]
fn closing_during_the_check_says_nothing_and_frees_the_flight() {
    let run = setup(UpdateDecision::UpdateNow, true);
    run.prompter.closing.cancel();
    manual(&run, Some(newer()));
    manual(&run, None);
    assert_eq!(run.out.text(), "");
    assert!(lock(&run.calls).is_empty());
    assert!(run.prompter.flight.try_enter().is_some());
}
