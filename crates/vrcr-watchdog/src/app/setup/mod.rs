use std::cell::Cell;

use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Component, Ux};

use crate::hosts::HostsManager;
use crate::maintenance::SetupError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Changes {
    Hosts,
    Certificate,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Yes,
    No,
    Closed,
    TimedOut,
}

pub(super) fn approve(decision: Decision, ux: &Ux) -> bool {
    if decision == Decision::TimedOut {
        ux.log_warn(
            Component::Terminal,
            "The Windows permission dialog timed out.",
        );
    }
    decision == Decision::Yes
}

pub(super) fn run<T>(
    hosts: &HostsManager,
    repair: bool,
    closing: &CancellationToken,
    dialog: &dyn Fn(Changes) -> Decision,
    ux: &Ux,
    certificate: impl FnOnce(&dyn Fn() -> bool) -> T,
) -> (T, Result<bool, SetupError>) {
    let answer = Cell::new(None);
    let ask = |changes| {
        if closing.is_cancelled() {
            return false;
        }
        if let Some((shown, agreed)) = answer.get() {
            return agreed && (shown == Changes::Both || shown == changes);
        }
        let agreed = approve(dialog(changes), ux);
        answer.set(Some((changes, agreed)));
        agreed && !closing.is_cancelled()
    };
    hosts.setup_pass(
        repair,
        || {
            certificate(&|| {
                ask(if hosts.read_state() == Ok(false) {
                    Changes::Both
                } else {
                    Changes::Certificate
                })
            })
        },
        &|| ask(Changes::Hosts),
    )
}

#[cfg(test)]
mod tests;
