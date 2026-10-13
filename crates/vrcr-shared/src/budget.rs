use std::time::Duration;

use crate::wire::fallback;

pub const TOTAL: Duration = Duration::from_secs(28);
pub const OG_RESERVE: Duration = Duration::from_secs(5);
pub const RE_ASK_RESERVE: Duration = Duration::from_secs(5);
pub const OG_HARD_CAP: Duration = Duration::from_secs(300);
pub const RE_ASK_MIN_WINDOW_MS: i64 = 2250;
pub const NO_HINT_MIN_REMAINING_MS: i64 = 3000;

pub const TOTAL_MS: i64 = TOTAL.as_millis() as i64;

pub fn re_ask_fits(retry_at_elapsed_ms: Option<i64>) -> bool {
    retry_at_elapsed_ms.is_none_or(|at| TOTAL_MS - at >= RE_ASK_MIN_WINDOW_MS)
}

pub fn re_ask_is_redundant(server_reason: Option<&str>, retry_at_elapsed_ms: Option<i64>) -> bool {
    retry_at_elapsed_ms.is_none()
        && server_reason.is_some_and(|r| r.starts_with(fallback::VALIDATION_FAILED_PREFIX))
}

pub fn re_ask_delay_ms(elapsed_ms: i64, retry_at_elapsed_ms: Option<i64>) -> Option<i64> {
    match retry_at_elapsed_ms {
        None => (TOTAL_MS - elapsed_ms >= NO_HINT_MIN_REMAINING_MS).then_some(0),
        Some(at) => {
            let start = at.max(elapsed_ms);
            (TOTAL_MS - start >= RE_ASK_MIN_WINDOW_MS).then_some(start - elapsed_ms)
        }
    }
}

pub fn og_window(elapsed: Duration, re_ask_available: bool, retry_at_elapsed_ms: Option<i64>) -> Duration {
    if !re_ask_available {
        return OG_HARD_CAP;
    }
    let reserve = if re_ask_fits(retry_at_elapsed_ms) {
        RE_ASK_RESERVE
    } else {
        Duration::ZERO
    };
    TOTAL
        .saturating_sub(elapsed)
        .saturating_sub(reserve)
        .max(OG_RESERVE)
}

pub mod retry {
    use crate::wire::fallback;

    const RETRYABLE: [&str; 2] = [fallback::DISCOVERY_IN_PROGRESS, fallback::SERVER_UNREACHABLE];
    pub const MAX_RETRIES: u32 = 2;
    pub const MIN_BUDGET_FOR_RETRY_MS: i64 = 2000;

    pub fn should_retry(reason: Option<&str>, attempts_so_far: u32, remaining_budget_ms: i64) -> bool {
        attempts_so_far < MAX_RETRIES
            && remaining_budget_ms >= MIN_BUDGET_FOR_RETRY_MS
            && reason.is_some_and(|r| RETRYABLE.contains(&r))
    }

    pub fn next_delay_ms(attempt: u32) -> i64 {
        if attempt == 0 { 750 } else { 2250 }
    }

    pub fn retry_delay_ms(
        reason: Option<&str>,
        retry_after_ms: Option<i32>,
        attempt: u32,
        remaining_budget_ms: i64,
    ) -> Option<i64> {
        match (reason, retry_after_ms) {
            (Some(fallback::DISCOVERY_IN_PROGRESS), Some(hint)) => {
                let hint = i64::from(hint);
                (hint <= 5000.min(remaining_budget_ms - MIN_BUDGET_FOR_RETRY_MS)).then_some(hint)
            }
            _ => Some(next_delay_ms(attempt)),
        }
    }
}
