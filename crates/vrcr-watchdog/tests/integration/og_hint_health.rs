use std::time::Duration;

use crate::common::{ManualClock, T_2026_05_22, T_2026_08_21, T_2026_09_29};
use vrcr_shared::budget::retry;
use vrcr_shared::wire::og_reason;
use vrcr_watchdog::health::{self, HealthGate, Transition};
use vrcr_watchdog::og_hint::{self, OgFallbackHint};

const ABC: &str = "https://www.youtube.com/watch?v=abcdefghijk";
const DEF: &str = "https://www.youtube.com/watch?v=defghijklmn";
const CDN4: &str = "https://live.cdn4.io/ttfcvr/index.m3u8";
const SECS: fn(u64) -> Duration = Duration::from_secs;
const MINS: fn(u64) -> Duration = |m| Duration::from_secs(m * 60);

fn hint(clock: &ManualClock) -> OgFallbackHint {
    OgFallbackHint::new(clock.now())
}

#[test]
fn should_prefer_og_defaults_false_when_source_never_failed() {
    let clock = ManualClock::utc(T_2026_05_22);
    assert!(!hint(&clock).should_prefer_og(ABC));
}

#[test]
fn first_failure_arms_a_five_minute_lease() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    assert_eq!(h.record_load_failure(ABC), Some(MINS(5)));
    assert!(h.should_prefer_og(ABC));
    clock.advance(MINS(5) - SECS(1));
    assert!(h.should_prefer_og(ABC));
    clock.advance(SECS(2));
    assert!(!h.should_prefer_og(ABC));
    assert_eq!(h.live_entry_count(), 0);
}

#[test]
fn repeated_failures_escalate_to_fifteen_then_thirty_minutes() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    assert_eq!(h.record_load_failure(CDN4), Some(MINS(5)));
    clock.advance(MINS(6));
    assert_eq!(h.record_load_failure(CDN4), Some(MINS(15)));
    clock.advance(MINS(16));
    assert_eq!(h.record_load_failure(CDN4), Some(MINS(30)));
    clock.advance(MINS(31));
    assert_eq!(h.record_load_failure(CDN4), Some(MINS(30)));
    assert_eq!(h.strikes(CDN4), 4);
    clock.advance(MINS(29));
    assert!(h.should_prefer_og(CDN4));
}

#[test]
fn signals_for_one_failed_play_count_as_a_single_strike() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    h.record_load_failure(CDN4);
    clock.advance(SECS(5));
    assert_eq!(h.record_load_failure(CDN4), Some(MINS(5)));
    clock.advance(SECS(20));
    assert_eq!(h.record_load_failure(CDN4), Some(MINS(5)));
    assert_eq!(h.strikes(CDN4), 1);
}

#[test]
fn strikes_are_forgotten_after_two_quiet_hours() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    h.record_load_failure(ABC);
    clock.advance(MINS(10));
    h.record_load_failure(ABC);
    clock.advance(og_hint::STRIKE_MEMORY + SECS(1));
    assert_eq!(h.record_load_failure(ABC), Some(MINS(5)));
    assert_eq!(h.strikes(ABC), 1);
}

#[test]
fn record_load_failure_is_keyed_by_normalized_source_url() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    h.record_load_failure("https://youtu.be/abcdefghijk?si=tracking123");
    assert!(h.should_prefer_og(ABC));
    assert!(h.should_prefer_og("https://WWW.YouTube.com/live/abcdefghijk"));
    assert!(h.should_prefer_og("https://m.youtube.com/shorts/abcdefghijk?feature=share"));
    assert!(
        h.should_prefer_og("https://www.youtube.com/watch?v=abcdefghijk&utm_source=x&fbclid=y")
    );
    assert!(!h.should_prefer_og(DEF));
    h.record_load_failure("HTTPS://Live.CDN4.io/ttfcvr/index.m3u8#frag");
    assert!(h.should_prefer_og(CDN4));
    assert!(!h.should_prefer_og("https://live.cdn4.io/other/index.m3u8"));
}

#[test]
fn meaningful_query_parameters_keep_sources_apart() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    h.record_load_failure("https://example.com/play?id=1&utm_medium=a");
    assert!(h.should_prefer_og("https://example.com/play?id=1"));
    assert!(!h.should_prefer_og("https://example.com/play?id=2"));
}

#[test]
fn try_clear_removes_active_entry_and_resets_strikes() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    h.record_load_failure(ABC);
    clock.advance(MINS(6));
    h.record_load_failure(ABC);
    assert!(h.try_clear("https://youtu.be/abcdefghijk"));
    assert!(!h.should_prefer_og(ABC));
    assert!(!h.try_clear(ABC));
    assert_eq!(h.record_load_failure(ABC), Some(MINS(5)));
}

#[test]
fn record_load_failure_with_empty_url_is_a_no_op() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    assert_eq!(h.record_load_failure(""), None);
    assert_eq!(h.live_entry_count(), 0);
    assert!(!h.should_prefer_og(""));
}

#[test]
fn og_block_is_scoped_to_one_normalized_url() {
    let clock = ManualClock::utc(T_2026_09_29);
    let h = hint(&clock);
    assert!(h.record_og_blocked("https://youtu.be/abcdefghijk"));
    assert!(h.is_og_blocked(ABC));
    assert!(!h.is_og_blocked(DEF));
    assert!(!h.is_og_blocked("https://music.youtube.com/watch?v=ghijklmnopq"));
    assert!(!h.is_og_blocked("https://soundcloud.com/a/b"));
}

#[test]
fn blocked_and_hinted_source_prefers_our_resolver() {
    let clock = ManualClock::utc(T_2026_09_29);
    let h = hint(&clock);
    h.record_load_failure(ABC);
    h.record_load_failure(DEF);
    assert!(h.should_prefer_og(ABC));
    h.record_og_blocked("https://youtu.be/abcdefghijk");
    assert!(!h.should_prefer_og(ABC));
    assert!(h.should_prefer_og(DEF));
    assert_eq!(h.strikes(ABC), 1);
    clock.advance(og_hint::OG_BLOCKED_TTL + SECS(1));
    assert!(!h.is_og_blocked(ABC));
}

#[test]
fn hint_armed_after_an_og_block_still_defers_to_our_resolver() {
    let clock = ManualClock::utc(T_2026_09_29);
    let h = hint(&clock);
    h.record_og_blocked(ABC);
    assert_eq!(h.record_load_failure(ABC), Some(MINS(5)));
    assert!(!h.should_prefer_og(ABC));
    assert!(h.remaining(ABC).is_some());
}

#[test]
fn record_og_blocked_reports_only_the_first_block_and_refreshes_expiry() {
    let clock = ManualClock::utc(T_2026_09_29);
    let h = hint(&clock);
    assert!(h.record_og_blocked("https://youtu.be/abcdefghijk"));
    clock.advance(og_hint::OG_BLOCKED_TTL - SECS(60));
    assert!(!h.record_og_blocked(ABC));
    clock.advance(SECS(120));
    assert!(h.is_og_blocked("https://youtu.be/abcdefghijk"));
}

#[test]
fn record_og_blocked_ignores_urls_without_a_host() {
    for url in ["", "not a url", "file:///C:/video.mp4"] {
        let h = OgFallbackHint::default();
        assert!(!h.record_og_blocked(url), "{url}");
        assert!(!h.is_og_blocked(url), "{url}");
    }
}

#[test]
fn leases_follow_the_strike_count() {
    assert_eq!(og_hint::lease_for(0), MINS(5));
    assert_eq!(og_hint::lease_for(1), MINS(5));
    assert_eq!(og_hint::lease_for(2), MINS(15));
    assert_eq!(og_hint::lease_for(3), MINS(30));
    assert_eq!(og_hint::lease_for(9), MINS(30));
}

#[test]
fn hint_map_stays_bounded() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock);
    for i in 0..(og_hint::MAX_ENTRIES + 50) {
        h.record_load_failure(&format!("https://example.com/v{i}.mp4"));
        clock.advance(SECS(1));
    }
    assert!(h.live_entry_count() <= og_hint::MAX_ENTRIES);
    assert!(h.should_prefer_og(&format!(
        "https://example.com/v{}.mp4",
        og_hint::MAX_ENTRIES + 49
    )));
}

const COOLDOWN: Duration = Duration::from_secs(30);
const VERDICT: Duration = Duration::from_secs(25);

fn gate() -> (ManualClock, HealthGate) {
    let clock = ManualClock::utc(T_2026_08_21);
    let g = HealthGate::new(COOLDOWN, VERDICT, clock.now());
    (clock, g)
}

fn open(g: &HealthGate) {
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: false,
        resolved: false,
    });
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: false,
        resolved: false,
    });
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: false,
            resolved: false
        }),
        Transition::Opened
    );
}

fn short(g: &HealthGate, connected: bool) -> bool {
    g.should_short_circuit(connected).0
}

fn past_cooldown(clock: &ManualClock) {
    clock.advance(COOLDOWN + SECS(1));
}

#[test]
fn opens_on_third_consecutive_resolve_failure() {
    let (_, g) = gate();
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: false,
            resolved: false
        }),
        Transition::None
    );
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: false,
            resolved: false
        }),
        Transition::None
    );
    assert!(!short(&g, true));
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: false,
            resolved: false
        }),
        Transition::Opened
    );
    assert!(short(&g, true));
}

#[test]
fn healthy_verdict_resets_resolve_streak() {
    let (_, g) = gate();
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: false,
        resolved: false,
    });
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: false,
        resolved: false,
    });
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: true,
    });
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: false,
        resolved: false,
    });
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: false,
        resolved: false,
    });
    assert!(!short(&g, true));
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: false,
            resolved: false
        }),
        Transition::Opened
    );
}

#[test]
fn no_probe_while_mesh_disconnected() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(short(&g, false));
    assert!(!short(&g, true));
}

#[test]
fn cooldown_gates_the_probe_and_concurrent_calls_stay_short_circuited() {
    let (clock, g) = gate();
    open(&g);
    assert!(short(&g, true));
    past_cooldown(&clock);
    assert!(!short(&g, true));
    assert!(short(&g, true));
}

#[test]
fn unhealthy_probe_reopens_with_fresh_cooldown() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: false,
            resolved: false
        }),
        Transition::None
    );
    clock.advance(SECS(29));
    assert!(short(&g, true));
    clock.advance(SECS(2));
    assert!(!short(&g, true));
}

#[test]
fn healthy_fallback_probe_closes_immediately() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: true,
            resolved: false
        }),
        Transition::Closed
    );
    assert!(!short(&g, true));
}

#[test]
fn resolved_probe_waits_for_playback_confirmation() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    assert_eq!(
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: true,
            resolved: true
        }),
        Transition::None
    );
    assert!(short(&g, true));
    assert_eq!(g.record_playback_confirmed(), Transition::Closed);
    assert!(!short(&g, true));
}

#[test]
fn resolved_probe_playback_failure_reopens() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: true,
    });
    assert_eq!(g.record_playback_failure(), Transition::None);
    assert!(short(&g, true));
    past_cooldown(&clock);
    assert!(!short(&g, true));
}

#[test]
fn verdict_timeout_closes_the_gate() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: true,
    });
    assert!(short(&g, true));
    clock.advance(VERDICT + SECS(1));
    assert_eq!(g.should_short_circuit(true), (false, Transition::Closed));
}

#[test]
fn playback_streak_opens_despite_healthy_resolves() {
    let (_, g) = gate();
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: true,
    });
    assert_eq!(g.record_playback_failure(), Transition::None);
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: true,
    });
    assert_eq!(g.record_playback_failure(), Transition::None);
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: true,
    });
    assert_eq!(g.record_playback_failure(), Transition::Opened);
    assert!(short(&g, true));
}

#[test]
fn confirmed_playback_resets_playback_streak() {
    let (_, g) = gate();
    g.record_playback_failure();
    g.record_playback_failure();
    assert_eq!(g.record_playback_confirmed(), Transition::None);
    g.record_playback_failure();
    g.record_playback_failure();
    assert!(!short(&g, true));
    assert_eq!(g.record_playback_failure(), Transition::Opened);
}

#[test]
fn confirmed_playback_closes_the_gate_during_cooldown() {
    let (_, g) = gate();
    open(&g);
    assert!(short(&g, true));
    assert_eq!(g.record_playback_confirmed(), Transition::Closed);
    assert!(!short(&g, true));
}

#[test]
fn gate_reasons_are_not_retryable() {
    for reason in [
        og_reason::RESOLVER_UNHEALTHY,
        og_reason::RESOLVED_URL_REJECTED,
    ] {
        assert!(!retry::should_retry(Some(reason), 0, 20_000), "{reason}");
    }
}

#[test]
fn awaiting_probe_verdict_is_false_until_the_probe_is_admitted() {
    let (clock, g) = gate();
    assert!(!g.awaiting_probe_verdict());
    open(&g);
    assert!(!g.awaiting_probe_verdict());
    past_cooldown(&clock);
    assert!(!g.awaiting_probe_verdict());
    assert!(short(&g, false));
    assert!(!g.awaiting_probe_verdict());
    assert!(!short(&g, true));
    assert!(g.awaiting_probe_verdict());
}

#[test]
fn awaiting_probe_verdict_stays_true_until_playback_is_confirmed() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: true,
    });
    assert!(g.awaiting_probe_verdict());
    g.record_playback_confirmed();
    assert!(!g.awaiting_probe_verdict());
}

#[test]
fn awaiting_probe_verdict_ends_when_the_probe_outcome_arrives() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: false,
        resolved: false,
    });
    assert!(!g.awaiting_probe_verdict());

    past_cooldown(&clock);
    assert!(!short(&g, true));
    g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
        healthy: true,
        resolved: false,
    });
    assert!(!g.awaiting_probe_verdict());

    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    g.record_playback_failure();
    assert!(!g.awaiting_probe_verdict());
}

#[test]
fn awaiting_probe_verdict_ends_when_the_verdict_timeout_lapses() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    clock.advance(VERDICT - SECS(1));
    assert!(g.awaiting_probe_verdict());
    clock.advance(SECS(1));
    assert!(!g.awaiting_probe_verdict());
}

#[test]
fn probe_that_never_reports_stops_blocking_after_verdict_timeout() {
    let (clock, g) = gate();
    g.record_playback_failure();
    g.record_playback_failure();
    assert_eq!(g.record_playback_failure(), Transition::Opened);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    assert!(short(&g, true));
    clock.advance(VERDICT);
    assert_eq!(g.should_short_circuit(true), (false, Transition::Closed));
}

#[test]
fn default_gate_constants_match_the_old_client() {
    assert_eq!(health::OPEN_THRESHOLD, 3);
    assert_eq!(health::DEFAULT_COOLDOWN, SECS(30));
    assert_eq!(health::DEFAULT_VERDICT_TIMEOUT, SECS(25));
}
