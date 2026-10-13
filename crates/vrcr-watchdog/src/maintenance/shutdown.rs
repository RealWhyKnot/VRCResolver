use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

pub const TOTAL_BUDGET: Duration = Duration::from_secs(12);

pub type StepFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
pub type StepRun = Box<dyn FnOnce() -> StepFuture + Send>;
pub type RestoreTools = Arc<dyn Fn(&str) -> Result<bool, String> + Send + Sync>;
pub type StepsProvider = Arc<dyn Fn() -> Vec<ShutdownStep> + Send + Sync>;
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ShutdownStep {
    pub name: String,
    pub run: StepRun,
    pub budget: Duration,
}

impl ShutdownStep {
    pub fn new<F, Fut>(name: &str, budget: Duration, run: F) -> ShutdownStep
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        ShutdownStep {
            name: name.to_owned(),
            run: Box::new(move || Box::pin(run())),
            budget,
        }
    }
}

pub struct ShutdownCoordinator {
    restore: RestoreTools,
    steps: StepsProvider,
    log: LogSink,
    total_budget: Duration,
    done: Mutex<Option<watch::Receiver<bool>>>,
}

pub fn panic_text(e: &tokio::task::JoinError) -> String {
    if e.is_cancelled() {
        return "cancelled".to_owned();
    }
    "panicked".to_owned()
}

impl ShutdownCoordinator {
    pub fn new(restore: RestoreTools, steps: StepsProvider, log: LogSink) -> ShutdownCoordinator {
        ShutdownCoordinator {
            restore,
            steps,
            log,
            total_budget: TOTAL_BUDGET,
            done: Mutex::new(None),
        }
    }

    pub fn with_total_budget(mut self, budget: Duration) -> ShutdownCoordinator {
        self.total_budget = budget;
        self
    }

    pub fn started(&self) -> bool {
        self.done.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    pub fn run(&self, trigger: &str, fast: bool) -> impl Future<Output = ()> + Send + 'static {
        let mut rx = {
            let mut guard = self.done.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(rx) => rx.clone(),
                None => {
                    let (tx, rx) = watch::channel(false);
                    let core = Core {
                        restore: self.restore.clone(),
                        steps: self.steps.clone(),
                        log: self.log.clone(),
                        total_budget: self.total_budget,
                    };
                    let trigger = trigger.to_owned();
                    tokio::spawn(async move {
                        core.run(&trigger, fast).await;
                        let _ = tx.send(true);
                    });
                    *guard = Some(rx.clone());
                    rx
                }
            }
        };
        async move {
            let _ = rx.wait_for(|done| *done).await;
        }
    }
}

struct Core {
    restore: RestoreTools,
    steps: StepsProvider,
    log: LogSink,
    total_budget: Duration,
}

impl Core {
    fn log(&self, line: &str) {
        let log = self.log.clone();
        let line = line.to_owned();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || log(&line)));
    }

    async fn run(&self, trigger: &str, fast: bool) {
        let clock = Instant::now();
        self.log(&format!("shutdown trigger={trigger}"));

        let restore = self.restore.clone();
        let reason = trigger.to_owned();
        let restored = match tokio::task::spawn_blocking(move || restore(&reason)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                self.log(&format!("tools restore threw {e}"));
                false
            }
            Err(e) => {
                self.log(&format!("tools restore threw {}", panic_text(&e)));
                false
            }
        };
        self.log(&format!("tools restored={restored}"));
        if fast {
            return;
        }

        let provider = self.steps.clone();
        let steps = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || provider())) {
            Ok(s) => s,
            Err(_) => {
                self.log("shutdown steps unavailable: panicked");
                return;
            }
        };

        for step in steps {
            let remaining = self.total_budget.saturating_sub(clock.elapsed());
            if remaining.is_zero() {
                self.log(&format!(
                    "shutdown budget spent; skipping {} and the rest",
                    step.name
                ));
                return;
            }
            self.run_step(step, remaining).await;
        }
    }

    async fn run_step(&self, step: ShutdownStep, remaining: Duration) {
        let budget = step.budget.min(remaining);
        let name = step.name;
        let run = step.run;
        let task = tokio::spawn(async move { run().await });
        match tokio::time::timeout(budget, task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => self.log(&format!("shutdown step {name} failed: {e}")),
            Ok(Err(e)) => self.log(&format!("shutdown step {name} failed: {}", panic_text(&e))),
            Err(_) => self.log(&format!(
                "shutdown step {name} took longer than {} ms; moving on",
                budget.as_millis()
            )),
        }
    }
}
