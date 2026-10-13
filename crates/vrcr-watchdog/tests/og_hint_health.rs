mod common;

use std::time::Duration;

use common::{ManualClock, T_2026_05_22, T_2026_08_21, T_2026_09_29};
use vrcr_shared::budget::retry;
use vrcr_shared::wire::og_reason;
use vrcr_watchdog::health::{self, HealthGate, Transition};
use vrcr_watchdog::og_hint::{self, OgFallbackHint};

const ABC: &str = "https://www.youtube.com/watch?v=abc";
const SECS: fn(u64) -> Duration = Duration::from_secs;

fn hint(clock: &ManualClock, ttl: Duration) -> OgFallbackHint {
    OgFallbackHint::new(ttl, clock.now())
}

#[test]
fn should_prefer_og_defaults_false_when_source_never_failed() {
    let clock = ManualClock::utc(T_2026_05_22);
    assert!(!hint(&clock, SECS(60)).should_prefer_og(ABC));
}

#[test]
fn should_prefer_og_true_within_ttl_after_recorded_failure() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock, SECS(60));
    h.record_load_failure(ABC);
    assert!(h.should_prefer_og(ABC));
    clock.advance(SECS(30));
    assert!(h.should_prefer_og(ABC));
}

#[test]
fn should_prefer_og_false_after_ttl_and_dropped_from_map() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock, SECS(60));
    h.record_load_failure(ABC);
    clock.advance(SECS(61));
    assert!(!h.should_prefer_og(ABC));
    assert_eq!(h.live_entry_count(), 0);
}

#[test]
fn record_load_failure_rearms_expiry_on_repeated_failure() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock, SECS(60));
    h.record_load_failure(ABC);
    clock.advance(SECS(45));
    h.record_load_failure(ABC);
    clock.advance(SECS(45));
    assert!(h.should_prefer_og(ABC));
}

#[test]
fn record_load_failure_is_keyed_by_source_url() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock, SECS(60));
    h.record_load_failure(ABC);
    assert!(!h.should_prefer_og("https://www.youtube.com/watch?v=def"));
    assert!(h.should_prefer_og(ABC));
}

#[test]
fn try_clear_removes_active_entry() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock, SECS(60));
    h.record_load_failure(ABC);
    assert!(h.try_clear(ABC));
    assert!(!h.should_prefer_og(ABC));
    assert!(!h.try_clear(ABC));
}

#[test]
fn record_load_failure_with_empty_url_is_a_no_op() {
    let clock = ManualClock::utc(T_2026_05_22);
    let h = hint(&clock, SECS(60));
    h.record_load_failure("");
    assert_eq!(h.live_entry_count(), 0);
    assert!(!h.should_prefer_og(""));
}

#[test]
fn og_block_covers_every_youtube_host_and_only_youtube() {
    let clock = ManualClock::utc(T_2026_09_29);
    let h = hint(&clock, SECS(60));
    assert!(h.record_og_blocked("https://youtu.be/abc"));
    assert!(h.is_og_blocked("https://www.youtube.com/watch?v=def"));
    assert!(h.is_og_blocked("https://music.youtube.com/watch?v=ghi"));
    assert!(h.is_og_blocked("https://m.youtube.com/watch?v=jkl"));
    assert!(!h.is_og_blocked("https://soundcloud.com/a/b"));
    assert!(!h.is_og_blocked("https://notyoutube.com/v"));
}

#[test]
fn og_block_overrides_the_per_video_hint_until_it_expires() {
    let clock = ManualClock::utc(T_2026_09_29);
    let h = hint(&clock, SECS(3600));
    h.record_load_failure("https://youtu.be/abc");
    h.record_og_blocked("https://youtu.be/abc");
    assert!(!h.should_prefer_og("https://youtu.be/abc"));
    clock.advance(og_hint::OG_BLOCKED_TTL + SECS(1));
    assert!(!h.is_og_blocked("https://youtu.be/abc"));
    assert!(h.should_prefer_og("https://youtu.be/abc"));
}

#[test]
fn record_og_blocked_reports_only_the_first_block_and_refreshes_expiry() {
    let clock = ManualClock::utc(T_2026_09_29);
    let h = hint(&clock, SECS(60));
    assert!(h.record_og_blocked("https://youtu.be/abc"));
    clock.advance(og_hint::OG_BLOCKED_TTL - SECS(60));
    assert!(!h.record_og_blocked("https://www.youtube.com/watch?v=def"));
    clock.advance(SECS(120));
    assert!(h.is_og_blocked("https://youtu.be/abc"));
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
fn default_ttl_is_sixty_seconds() {
    assert_eq!(OgFallbackHint::default().ttl(), SECS(60));
}

const COOLDOWN: Duration = Duration::from_secs(30);
const VERDICT: Duration = Duration::from_secs(25);

fn gate() -> (ManualClock, HealthGate) {
    let clock = ManualClock::utc(T_2026_08_21);
    let g = HealthGate::new(COOLDOWN, VERDICT, clock.now());
    (clock, g)
}

fn open(g: &HealthGate) {
    g.record_resolve_outcome(false, false);
    g.record_resolve_outcome(false, false);
    assert_eq!(g.record_resolve_outcome(false, false), Transition::Opened);
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
    assert_eq!(g.record_resolve_outcome(false, false), Transition::None);
    assert_eq!(g.record_resolve_outcome(false, false), Transition::None);
    assert!(!short(&g, true));
    assert_eq!(g.record_resolve_outcome(false, false), Transition::Opened);
    assert!(short(&g, true));
}

#[test]
fn healthy_verdict_resets_resolve_streak() {
    let (_, g) = gate();
    g.record_resolve_outcome(false, false);
    g.record_resolve_outcome(false, false);
    g.record_resolve_outcome(true, true);
    g.record_resolve_outcome(false, false);
    g.record_resolve_outcome(false, false);
    assert!(!short(&g, true));
    assert_eq!(g.record_resolve_outcome(false, false), Transition::Opened);
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
    assert_eq!(g.record_resolve_outcome(false, false), Transition::None);
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
    assert_eq!(g.record_resolve_outcome(true, false), Transition::Closed);
    assert!(!short(&g, true));
}

#[test]
fn resolved_probe_waits_for_playback_confirmation() {
    let (clock, g) = gate();
    open(&g);
    past_cooldown(&clock);
    assert!(!short(&g, true));
    assert_eq!(g.record_resolve_outcome(true, true), Transition::None);
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
    g.record_resolve_outcome(true, true);
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
    g.record_resolve_outcome(true, true);
    assert!(short(&g, true));
    clock.advance(VERDICT + SECS(1));
    assert_eq!(g.should_short_circuit(true), (false, Transition::Closed));
}

#[test]
fn playback_streak_opens_despite_healthy_resolves() {
    let (_, g) = gate();
    g.record_resolve_outcome(true, true);
    assert_eq!(g.record_playback_failure(), Transition::None);
    g.record_resolve_outcome(true, true);
    assert_eq!(g.record_playback_failure(), Transition::None);
    g.record_resolve_outcome(true, true);
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
    for reason in [og_reason::RESOLVER_UNHEALTHY, og_reason::RESOLVED_URL_REJECTED] {
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
    g.record_resolve_outcome(true, true);
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
    g.record_resolve_outcome(false, false);
    assert!(!g.awaiting_probe_verdict());

    past_cooldown(&clock);
    assert!(!short(&g, true));
    g.record_resolve_outcome(true, false);
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
