use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use vrcr_shared::win::CtrlEvent;

use super::CLOSE_WAIT;
use crate::maintenance::shutdown::ShutdownCoordinator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Close {
    Open,
    Signalled,
    Finished,
}

pub(super) struct Lifecycle {
    handle: Handle,
    pub(super) coordinator: OnceLock<ShutdownCoordinator>,
    pub(super) started: AtomicBool,
    pub(super) fast: AtomicBool,
    closed: Mutex<Close>,
    wake: Condvar,
    pub(super) closing: CancellationToken,
}

impl Lifecycle {
    pub(super) fn new(handle: Handle) -> Lifecycle {
        Lifecycle {
            handle,
            coordinator: OnceLock::new(),
            started: AtomicBool::new(false),
            fast: AtomicBool::new(false),
            closed: Mutex::new(Close::Open),
            wake: Condvar::new(),
            closing: CancellationToken::new(),
        }
    }

    fn begin(
        &self,
        trigger: &str,
        fast: bool,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let coordinator = self.coordinator.get()?;
        if !self.started.swap(true, Ordering::SeqCst) {
            self.fast.store(fast, Ordering::SeqCst);
            self.closing.cancel();
        }
        let _entered = self.handle.enter();
        Some(coordinator.run(trigger, fast))
    }

    pub(super) fn shutdown(&self, trigger: &str, fast: bool, limit: Option<Duration>) {
        let Some(done) = self.begin(trigger, fast) else {
            return;
        };
        match limit {
            Some(limit) => {
                let _ = self
                    .handle
                    .block_on(async { tokio::time::timeout(limit, done).await });
            }
            None => self.handle.block_on(done),
        }
    }

    fn close_state(&self) -> std::sync::MutexGuard<'_, Close> {
        self.closed.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn signal_closed(&self) {
        let mut state = self.close_state();
        if *state == Close::Open {
            *state = Close::Signalled;
        }
        drop(state);
        self.wake.notify_all();
    }

    pub(super) fn finish(&self) {
        *self.close_state() = Close::Finished;
        self.wake.notify_all();
    }

    pub(super) fn wait_for_close(&self) {
        let guard = self.close_state();
        let _guard = self
            .wake
            .wait_while(guard, |state| *state == Close::Open)
            .unwrap_or_else(|e| e.into_inner());
    }

    fn wait_for_finish(&self, limit: Duration) {
        let guard = self.close_state();
        let _guard = self
            .wake
            .wait_timeout_while(guard, limit, |state| *state != Close::Finished)
            .unwrap_or_else(|e| e.into_inner());
    }

    fn is_closing(&self) -> bool {
        self.started.load(Ordering::SeqCst) && *self.close_state() != Close::Open
    }

    pub(super) fn on_ctrl(&self, event: CtrlEvent) {
        if self.is_closing() {
            if event != CtrlEvent::CtrlC {
                self.wait_for_finish(CLOSE_WAIT);
            }
            return;
        }
        let trigger = match event {
            CtrlEvent::CtrlC => {
                drop(self.begin("ctrl-c", false));
                self.signal_closed();
                return;
            }
            CtrlEvent::Close => "console-close",
            CtrlEvent::Logoff => "logoff",
            CtrlEvent::Shutdown => "system-shutdown",
        };
        self.shutdown(trigger, true, Some(CLOSE_WAIT));
        self.signal_closed();
    }
}
