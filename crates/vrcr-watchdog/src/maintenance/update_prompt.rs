use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Color, Component, Ux};

use super::update::{CheckOutcome, Release, Spawn, UPDATER_EXE, UpdateGate, UpdateStart, check};
use super::update_dialog::UpdateDecision;
use crate::settings::Store;

pub type Prompt = Arc<dyn Fn(&Release, &str, &CancellationToken) -> UpdateDecision + Send + Sync>;

const FLIGHT_IDLE: u8 = 0;
const FLIGHT_CHECKING: u8 = 1;
const FLIGHT_PROMPTING: u8 = 2;

#[derive(Clone, Default)]
pub struct CheckFlight {
    state: Arc<AtomicU8>,
}

pub struct CheckTicket {
    state: Arc<AtomicU8>,
}

impl CheckTicket {
    pub fn mark_prompting(&self) {
        self.state.store(FLIGHT_PROMPTING, Ordering::Release);
    }
}

impl Drop for CheckTicket {
    fn drop(&mut self) {
        self.state.store(FLIGHT_IDLE, Ordering::Release);
    }
}

impl CheckFlight {
    pub fn try_enter(&self) -> Option<CheckTicket> {
        self.state
            .compare_exchange(
                FLIGHT_IDLE,
                FLIGHT_CHECKING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        Some(CheckTicket {
            state: Arc::clone(&self.state),
        })
    }

    pub fn busy_reason(&self) -> CheckStart {
        if self.state.load(Ordering::Acquire) == FLIGHT_PROMPTING {
            CheckStart::Prompting
        } else {
            CheckStart::Checking
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStart {
    Started,
    Checking,
    Prompting,
    Installing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Startup,
    Manual,
}

#[derive(Clone)]
pub struct Prompter {
    pub settings: Arc<Store>,
    pub gate: UpdateGate,
    pub flight: CheckFlight,
    pub install_dir: PathBuf,
    pub spawn: Spawn,
    pub current: String,
    pub interactive: bool,
    pub closing: CancellationToken,
    pub ux: Ux,
    pub prompt: Prompt,
}

pub fn normalize_version(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

fn same_version(left: &str, right: &str) -> bool {
    normalize_version(left).eq_ignore_ascii_case(normalize_version(right))
}

pub async fn check_and_offer<F, Fut>(include_prereleases: bool, prompter: Prompter, fetch: F)
where
    F: FnOnce(&'static str) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let Some(ticket) = prompter.flight.try_enter() else {
        return;
    };
    run_check(
        Trigger::Startup,
        include_prereleases,
        prompter,
        ticket,
        fetch,
    )
    .await;
}

pub async fn check_and_offer_manual<F, Fut>(
    include_prereleases: bool,
    prompter: Prompter,
    ticket: CheckTicket,
    fetch: F,
) where
    F: FnOnce(&'static str) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    run_check(
        Trigger::Manual,
        include_prereleases,
        prompter,
        ticket,
        fetch,
    )
    .await;
}

async fn run_check<F, Fut>(
    trigger: Trigger,
    include_prereleases: bool,
    prompter: Prompter,
    ticket: CheckTicket,
    fetch: F,
) where
    F: FnOnce(&'static str) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let outcome = check(include_prereleases, &prompter.current, fetch).await;
    if prompter.closing.is_cancelled() {
        return;
    }
    let release = match outcome {
        CheckOutcome::UpdateAvailable(release) => release,
        CheckOutcome::UpToDate => {
            if trigger == Trigger::Manual {
                prompter.say(&format!(
                    "You're already on the latest version ({}).",
                    normalize_version(&prompter.current)
                ));
            }
            return;
        }
        CheckOutcome::Failed(reason) => {
            prompter
                .ux
                .log_warn(Component::Update, &format!("update check failed: {reason}"));
            if trigger == Trigger::Manual {
                prompter.ux.notice(
                    Color::Yellow,
                    "Couldn't check for updates. Try again in a moment.",
                );
            }
            return;
        }
    };
    let ux = prompter.ux.clone();
    ticket.mark_prompting();
    let result = tokio::task::spawn_blocking(move || match trigger {
        Trigger::Startup => prompter.offer(&release),
        Trigger::Manual => prompter.offer_manual(&release),
    })
    .await;
    drop(ticket);
    if let Err(error) = result {
        ux.log_warn(
            Component::Update,
            &format!("update prompt stopped unexpectedly: {error}"),
        );
    }
}

impl Prompter {
    pub fn offer(&self, release: &Release) -> Option<UpdateDecision> {
        let version = normalize_version(&release.tag);
        let skipped = self.settings.skipped_version();
        if skipped.is_some_and(|skipped| same_version(&skipped, version)) {
            self.say(&format!(
                "Version {version} is available, but you skipped it. Type update to review it."
            ));
            return None;
        }
        self.present(release, Trigger::Startup)
    }

    pub fn offer_manual(&self, release: &Release) -> Option<UpdateDecision> {
        self.clear_skipped_version();
        self.present(release, Trigger::Manual)
    }

    fn clear_skipped_version(&self) {
        if self.settings.skipped_version().is_none() {
            return;
        }
        if let Err(error) = self.settings.set_skipped_version(None) {
            self.ux.log_warn(
                Component::Update,
                &format!("could not clear the skipped version: {error}"),
            );
        }
    }

    fn present(&self, release: &Release, trigger: Trigger) -> Option<UpdateDecision> {
        let version = normalize_version(&release.tag);
        if !self.install_dir.join(UPDATER_EXE).is_file() {
            self.ux.log_warn(
                Component::Update,
                "updater binary is missing; skipping the update prompt",
            );
            self.warn_missing_updater(release);
            return None;
        }
        if !self.interactive {
            return match trigger {
                Trigger::Startup => {
                    release.announce(&self.ux);
                    None
                }
                Trigger::Manual => {
                    self.update_now(version);
                    Some(UpdateDecision::UpdateNow)
                }
            };
        }
        self.say(&format!("Version {version} is available."));
        if self.closing.is_cancelled() {
            return None;
        }
        let decision = (self.prompt)(release, normalize_version(&self.current), &self.closing);
        if self.closing.is_cancelled() {
            return None;
        }
        match decision {
            UpdateDecision::UpdateNow => self.update_now(version),
            UpdateDecision::Later => {
                self.say("Update postponed. Type update anytime to install it.")
            }
            UpdateDecision::SkipVersion => self.skip(version),
        }
        Some(decision)
    }

    fn update_now(&self, version: &str) {
        match self.gate.start(&self.install_dir, self.spawn) {
            UpdateStart::Started => self.say(&format!("Starting the update to {version}.")),
            UpdateStart::AlreadyRunning => self.say("An update is already running."),
            UpdateStart::UpdaterMissing => {
                self.warn("Couldn't start the update because the updater is missing.");
            }
            UpdateStart::Failed => self.warn("Couldn't start the update."),
        }
    }

    fn skip(&self, version: &str) {
        match self.settings.set_skipped_version(Some(version)) {
            Ok(()) => self.say(&format!(
                "You skipped version {version}. Type update to review it."
            )),
            Err(error) => {
                self.ux.log_warn(
                    Component::Update,
                    &format!("could not save skipped version {version}: {error}"),
                );
                self.ux.notice(
                    Color::Yellow,
                    "Couldn't save your choice to skip this version.",
                );
            }
        }
    }

    fn say(&self, sentence: &str) {
        self.ux.log_success(Component::Update, sentence);
        self.ux.notice(Color::White, sentence);
    }

    fn warn(&self, sentence: &str) {
        self.ux.log_warn(Component::Update, sentence);
        self.ux.notice(Color::Yellow, sentence);
    }

    fn warn_missing_updater(&self, release: &Release) {
        let sentence = format!(
            "Version {} is available, but the updater is missing. Download the new version from",
            normalize_version(&release.tag)
        );
        let link = release.notes_url();
        self.ux
            .log_warn(Component::Update, &format!("{sentence} {link}"));
        self.ux.notice_with_link(Color::Yellow, &sentence, &link);
    }
}
