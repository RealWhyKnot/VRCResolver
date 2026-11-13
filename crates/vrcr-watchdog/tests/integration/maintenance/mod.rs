use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::common::mesh::Capture;
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use vrcr_shared::console::Ux;
use vrcr_shared::log::Log;
use vrcr_shared::wire;
use vrcr_watchdog::maintenance::codec::{
    self, Codec, CodecCapabilities, CodecEntry, CodecEnv, CodecOutcome, CodecState, Installer,
    WingetResult,
};
use vrcr_watchdog::maintenance::shutdown::{ShutdownCoordinator, ShutdownStep, cancel_all};
use vrcr_watchdog::maintenance::update::{self, Release, UpdateStart, Version};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (out.ux(), out)
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| panic!("{e}"))
        .block_on(f)
}

const HEVC: Codec = Codec {
    name: "HEVC Video Extensions",
    package: "Microsoft.HEVCVideoExtension",
    probe: "h265",
    store_ids: &["paid-id", "free-id"],
};

fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(crate::common::T_2026_08_21)
}

type Winget = Box<dyn Fn(&str) -> WingetResult + Send + Sync>;

struct Env {
    mf: Box<dyn Fn(&str) -> bool + Send + Sync>,
    appx: Box<dyn Fn(&str) -> bool + Send + Sync>,
    winget: Winget,
    now: SystemTime,
    appx_calls: AtomicUsize,
    winget_calls: Mutex<Vec<String>>,
}

impl Env {
    fn new() -> Env {
        Env {
            mf: Box::new(|_| false),
            appx: Box::new(|_| false),
            winget: Box::new(|_| WingetResult::exited(1, "")),
            now: t0(),
            appx_calls: AtomicUsize::new(0),
            winget_calls: Mutex::new(Vec::new()),
        }
    }
}

impl CodecEnv for &Env {
    fn mf_probe(&self, codec: &str) -> bool {
        (self.mf)(codec)
    }

    async fn appx_probe(&self, package: &str) -> bool {
        self.appx_calls.fetch_add(1, Ordering::SeqCst);
        (self.appx)(package)
    }

    async fn winget_install(&self, store_id: &str) -> WingetResult {
        lock(&self.winget_calls).push(store_id.to_owned());
        (self.winget)(store_id)
    }

    fn now(&self) -> SystemTime {
        self.now
    }
}

fn run_codecs(env: &Env, state: &mut CodecState) -> (bool, Vec<CodecOutcome>) {
    let installer = Installer::new(env);
    let mut seen = Vec::new();
    let dirty = block_on(installer.run(&[HEVC], state, |o| seen.push(o.clone())));
    (dirty, seen)
}

fn failed(class: Option<&str>, ago: Duration) -> CodecEntry {
    CodecEntry {
        status: "failed".to_owned(),
        failure_class: class.map(str::to_owned),
        last_attempt_utc: t0() - ago,
        package: String::new(),
    }
}

fn pick(doc: &serde_json::Value) -> Option<Release> {
    update::pick_highest(doc)
}

struct ShutdownRig {
    log: Arc<Mutex<Vec<String>>>,
}

impl ShutdownRig {
    fn new() -> ShutdownRig {
        ShutdownRig {
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn coordinator(
        &self,
        restore: impl Fn(&str) -> Result<bool, String> + Send + Sync + 'static,
        steps: impl Fn() -> Vec<ShutdownStep> + Send + Sync + 'static,
    ) -> ShutdownCoordinator {
        let log = std::sync::Arc::clone(&self.log);
        ShutdownCoordinator::new(
            Arc::new(restore),
            Arc::new(steps),
            Arc::new(move |line: &str| lock(&log).push(line.to_owned())),
        )
    }

    fn lines(&self) -> Vec<String> {
        lock(&self.log).clone()
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap_or_else(|e| panic!("{e}"))
}

const GENEROUS: Duration = Duration::from_secs(5);

mod codec_state_cases;

mod codecs_cases;

mod contracts_cases;

mod shutdown_cases;

mod updates_cases;
