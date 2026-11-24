use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Color, Component, Ux};

use super::update::{Release, Spawn, UPDATER_EXE, UpdateGate, UpdateStart, check};
use super::update_dialog::UpdateDecision;
use crate::settings::Store;

pub type Prompt = Arc<dyn Fn(&str, &str, &CancellationToken) -> UpdateDecision + Send + Sync>;

#[derive(Clone)]
pub struct Prompter {
    pub settings: Arc<Store>,
    pub gate: UpdateGate,
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
    let Some(release) = check(include_prereleases, &prompter.current, fetch).await else {
        return;
    };
    let ux = prompter.ux.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || prompter.offer(&release)).await {
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
                "You skipped version {version}. Type update to install it."
            ));
            return None;
        }
        if !self.install_dir.join(UPDATER_EXE).is_file() {
            self.ux.log_warn(
                Component::Update,
                "updater binary is missing; skipping the update prompt",
            );
            release.announce(&self.ux);
            return None;
        }
        if !self.interactive {
            release.announce(&self.ux);
            return None;
        }
        self.say(&format!("Version {version} is available."));
        if self.closing.is_cancelled() {
            return None;
        }
        let decision = (self.prompt)(version, normalize_version(&self.current), &self.closing);
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
                "You skipped version {version}. Type update to install it."
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
}
