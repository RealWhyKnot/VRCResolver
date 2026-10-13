#![allow(clippy::unwrap_used)]

use std::borrow::Cow;

use vrcr_shared::budget::retry;
use vrcr_shared::profile;
use vrcr_shared::wire::{self, Player, ResolveRequest, action, fallback, og_reason};

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn height_cap_parses_vrchat_selector_caps() {
    assert_eq!(profile::height_cap(Some("(mp4/best)[height<=?720]")), Some(720));
    assert_eq!(profile::height_cap(Some("best[height<=1080]/best")), Some(1080));
    assert_eq!(
        profile::height_cap(Some("bv*[protocol^=m3u8_native][height<=?480]+ba")),
        Some(480)
    );
    for none in [None, Some(""), Some("best"), Some("best[height<=?]")] {
        assert_eq!(profile::height_cap(none), None, "{none:?}");
    }
}

#[test]
fn infer_player_treats_a_720_cap_as_unity() {
    assert_eq!(
        profile::infer_player(Some("(mp4/best)[height<=?720]")),
        Player::Unity
    );
    assert_eq!(
        profile::infer_player(Some("best[height<=720]/best")),
        Player::Unity
    );
    assert_eq!(
        profile::infer_player(Some("best[height<=1080]/best")),
        Player::AvPro
    );
    assert_eq!(profile::infer_player(None), Player::AvPro);
}

#[test]
fn dash_f_value_matches_spaced_forms_only() {
    assert_eq!(
        profile::dash_f_value(&args(&["-f", "best[height<=1080]", "https://x"])),
        Some("best[height<=1080]")
    );
    assert_eq!(
        profile::dash_f_value(&args(&["--format", "best", "https://x"])),
        Some("best")
    );
    assert_eq!(profile::dash_f_value(&args(&["https://x"])), None);
    assert_eq!(profile::dash_f_value(&args(&["https://x", "-f"])), None);
    assert_eq!(
        profile::dash_f_value(&args(&["--format=best", "https://x"])),
        None
    );
}

#[test]
fn wrapper_request_carries_everything_the_server_needs() {
    for (format, player, height) in [
        (
            "(mp4/best)[height<=?4096][height>=?64][width>=?64]",
            Player::AvPro,
            4096,
        ),
        (
            "(mp4/best)[height<=?720][height>=?64][width>=?64]",
            Player::Unity,
            720,
        ),
    ] {
        assert_eq!(profile::infer_player(Some(format)), player);
        let req = profile::wrapper_request(
            profile::new_request_id(),
            "https://youtu.be/x",
            player,
            Some(format),
            true,
        );
        assert_eq!(req.player, Some(player));
        assert_eq!(req.action, action::RESOLVE);
        assert_eq!(req.id.len(), 32);
        assert_eq!(req.url, "https://youtu.be/x");
        assert_eq!(req.max_height, Some(height));
        assert_eq!(req.vrchat_format_arg.as_deref(), Some(format));
        assert_eq!(req.protocol_version, Some(wire::CLIENT_PROTOCOL_VERSION));
        assert_eq!(req.skip_native_hint, Some(true));
        let unity = player == Player::Unity;
        let protocols: &[&str] = if unity {
            &wire::UNITY_ACCEPT_PROTOCOLS
        } else {
            &wire::AVPRO_ACCEPT_PROTOCOLS
        };
        let codecs: &[&str] = if unity {
            &wire::UNITY_ACCEPT_CODECS
        } else {
            &wire::AVPRO_ACCEPT_CODECS
        };
        assert_eq!(req.accept_protocols.as_deref().unwrap(), protocols);
        assert_eq!(req.accept_codecs.as_deref().unwrap(), codecs);
        let channels = if unity {
            wire::UNITY_MAX_AUDIO_CHANNELS
        } else {
            wire::AVPRO_MAX_AUDIO_CHANNELS
        };
        assert_eq!(req.max_audio_channels, Some(channels));
        let plain = profile::wrapper_request("x".into(), "https://youtu.be/x", player, Some(format), false);
        assert_eq!(plain.skip_native_hint, None);
    }
}

const VRCHAT_ARG: &str = "(mp4/best)[height<=?2160][height>=?64][width>=?64]";

#[test]
fn cap_height_in_lowers_only_higher_caps() {
    assert_eq!(
        profile::cap_height_in(VRCHAT_ARG, 1080),
        "(mp4/best)[height<=?1080][height>=?64][width>=?64]"
    );
    let low = "(mp4/best)[height<=?720][height>=?64]";
    assert!(matches!(profile::cap_height_in(low, 1080), Cow::Borrowed(s) if s == low));
    assert_eq!(
        profile::cap_height_in("bv[height<=4320]/bv[height<=1440]/bv[height<=480]", 1080),
        "bv[height<=1080]/bv[height<=1080]/bv[height<=480]"
    );
}

fn avpro(max_height: Option<i32>, format: Option<&str>) -> ResolveRequest {
    ResolveRequest {
        max_height,
        vrchat_format_arg: format.map(str::to_owned),
        ..ResolveRequest::for_player(
            "id-1".into(),
            "https://www.youtube.com/watch?v=x".into(),
            Player::AvPro,
        )
    }
}

#[test]
fn default_quality_cap_caps_avpro_in_both_places() {
    let mut req = avpro(Some(2160), Some(VRCHAT_ARG));
    assert!(profile::apply_default_quality_cap(&mut req));
    assert_eq!(req.max_height, Some(1080));
    assert_eq!(
        req.vrchat_format_arg.as_deref(),
        Some("(mp4/best)[height<=?1080][height>=?64][width>=?64]")
    );
}

#[test]
fn default_quality_cap_never_raises_and_fills_the_default() {
    let mut low = avpro(Some(480), None);
    assert!(!profile::apply_default_quality_cap(&mut low));
    assert_eq!(low.max_height, Some(480));
    let mut empty = avpro(None, None);
    assert!(profile::apply_default_quality_cap(&mut empty));
    assert_eq!(empty.max_height, Some(1080));
}

#[test]
fn default_quality_cap_and_high_quality_leave_unity_alone() {
    let mut req = avpro(Some(720), Some("(mp4/best)[height<=?720]"));
    req.player = Some(Player::Unity);
    assert!(!profile::apply_default_quality_cap(&mut req));
    assert!(!profile::apply_high_quality(&mut req, true));
    assert_eq!(req.max_height, Some(720));
    assert_eq!(req.vrchat_format_arg.as_deref(), Some("(mp4/best)[height<=?720]"));
    assert_eq!(req.prefer_highest, None);
}

#[test]
fn high_quality_raises_the_cap_and_drops_the_vrchat_selector() {
    let fmt = "bv*[height<=1080]+ba/best[height<=1080]";
    let mut off = avpro(Some(1080), Some(fmt));
    assert!(!profile::apply_high_quality(&mut off, false));
    assert_eq!(off.max_height, Some(1080));
    assert_eq!(off.vrchat_format_arg.as_deref(), Some(fmt));
    assert_eq!(off.prefer_highest, None);

    let mut on = avpro(Some(2160), Some(VRCHAT_ARG));
    assert!(profile::apply_high_quality(&mut on, true));
    assert_eq!(on.max_height, Some(wire::HIGH_QUALITY_MAX_HEIGHT));
    assert_eq!(on.vrchat_format_arg, None);
    assert_eq!(on.prefer_highest, Some(true));
}

#[test]
fn prefer_highest_serializes_only_when_set() {
    let req = avpro(Some(1080), None);
    let json = serde_json::to_string(&req).unwrap();
    assert!(!json.contains("prefer_highest"));
    let mut hq = avpro(Some(1080), Some("x"));
    profile::apply_high_quality(&mut hq, true);
    let json = serde_json::to_string(&hq).unwrap();
    assert!(json.contains("\"prefer_highest\":true"));
    assert!(json.contains("\"maxHeight\":2160"));
}

#[test]
fn retry_policy_retries_only_transient_reasons_within_budget() {
    let dip = Some(fallback::DISCOVERY_IN_PROGRESS);
    assert!(retry::should_retry(dip, 0, 15_000));
    assert!(!retry::should_retry(dip, retry::MAX_RETRIES, 15_000));
    assert!(!retry::should_retry(dip, 0, 1_500));
    assert!(retry::should_retry(dip, 0, retry::MIN_BUDGET_FOR_RETRY_MS));
    assert!(!retry::should_retry(dip, 0, retry::MIN_BUDGET_FOR_RETRY_MS - 1));
    assert!(retry::should_retry(Some(fallback::SERVER_UNREACHABLE), 0, 15_000));
    for reason in [
        fallback::ALL_CONFIGS_FAILED,
        fallback::CLIENT_DEADLINE_EXCEEDED,
        fallback::RATE_LIMITED,
        fallback::PROTOCOL_ERROR,
        og_reason::RESOLVER_UNHEALTHY,
        og_reason::RESOLVED_URL_REJECTED,
    ] {
        assert!(!retry::should_retry(Some(reason), 0, 30_000), "{reason}");
    }
    assert!(!retry::should_retry(None, 0, 15_000));
    assert_eq!(retry::MAX_RETRIES, 2);
    assert_eq!(retry::MIN_BUDGET_FOR_RETRY_MS, 2000);
}

#[test]
fn retry_delays_follow_the_ladder_and_the_server_hint() {
    let dip = Some(fallback::DISCOVERY_IN_PROGRESS);
    assert_eq!(retry::next_delay_ms(0), 750);
    assert_eq!(retry::next_delay_ms(1), 2250);
    assert_eq!(retry::retry_delay_ms(dip, Some(2000), 0, 24_000), Some(2000));
    assert_eq!(retry::retry_delay_ms(dip, Some(25_000), 0, 26_000), None);
    assert_eq!(retry::retry_delay_ms(dip, None, 0, 26_000), Some(750));
    assert_eq!(retry::retry_delay_ms(dip, Some(4800), 0, 6_000), None);
    assert_eq!(
        retry::retry_delay_ms(Some(fallback::SERVER_UNREACHABLE), Some(25_000), 1, 26_000),
        Some(2250)
    );
}
