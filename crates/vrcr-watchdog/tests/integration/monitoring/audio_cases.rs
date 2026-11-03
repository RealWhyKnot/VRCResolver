use super::*;

#[test]
fn a_closed_gate_announces_recovery() {
    let r = Rig::new().with_gate();
    let mut m = r.monitor();
    for _ in 0..3 {
        m.mark_playback_failure(PLAYBACK);
    }
    assert!(r.gate_open());
    r.clock.advance(health::DEFAULT_COOLDOWN + secs(1));
    let _ = r.gate.as_ref().map(|g| g.should_short_circuit(true));
    r.feed(
        &mut m,
        &format!(
            "[AVProVideo] Opening {PLAYBACK}\n[AVProVideo] Using playback path: MediaFoundation\n"
        ),
    );
    assert!(
        r.out
            .text()
            .contains("resolver restored -- mesh resolving re-enabled"),
        "{}",
        r.out.text()
    );
}

#[test]
fn audio_device_count_is_tracked_once_per_change() {
    let r = Rig::new();
    let mut m = r.monitor();
    let none = "2026.09.29 13:23:29 Debug      -  uSpeak: SetInputDevice 0 (0 total, index out of range, setting to default device) 'No device available'\n";
    r.feed(&mut m, none);
    assert!(m.audio_devices_missing());
    r.feed(&mut m, none);
    assert!(m.audio_devices_missing());
    r.feed(
        &mut m,
        "2026.09.29 13:26:28 Debug      -  uSpeak: SetInputDevice 0 (5 total) 'Microphone (2- fifine Microphone)'\n",
    );
    assert!(!m.audio_devices_missing());
    assert_eq!(r.probes.load(Ordering::SeqCst), 1);
    let text = r.out.text();
    assert_eq!(text.matches("VRChat found no audio devices").count(), 1);
    assert!(text.contains("VRChat's audio devices are back"));
}

#[test]
fn audio_helpers_match_the_csharp_tables() {
    assert_eq!(
        audio::no_sound_reason(&AudioCheck::new(
            AudioState::InUse,
            Some("Speakers (3- fifine Microphone)")
        )),
        "Windows' default playback device 'Speakers (3- fifine Microphone)' is held by another app in exclusive mode (Voicemeeter, ASIO or KS outputs can do this)"
    );
    assert!(
        audio::no_sound_reason(&AudioCheck::new(AudioState::InUse, None))
            .starts_with("Windows' default playback device is held")
    );
    assert_eq!(
        audio::no_sound_reason(&AudioCheck::new(AudioState::NoDevice, None)),
        "Windows has no default playback device"
    );
    for (code, state) in [
        (0, AudioState::Ok),
        (4, AudioState::InUse),
        (2, AudioState::NoDevice),
        (6, AudioState::NoDevice),
        (1, AudioState::Unknown),
    ] {
        assert_eq!(audio::classify(code), state, "{code}");
    }
    assert_eq!(
        audio::from_probe(DefaultOutput::NoDevices).state,
        AudioState::NoDevice
    );
    assert_eq!(
        audio::from_probe(DefaultOutput::NoPreferred).state,
        AudioState::NoDevice
    );
    assert_eq!(
        audio::from_probe(DefaultOutput::Unknown).state,
        AudioState::Unknown
    );
    assert_eq!(
        audio::from_probe(DefaultOutput::Opened {
            result: 4,
            device: Some("Out".to_owned())
        }),
        AudioCheck::new(AudioState::InUse, Some("Out"))
    );
    assert!(AudioCheck::new(AudioState::InUse, None).blocks_playback());
    assert!(!AudioCheck::new(AudioState::Unknown, None).blocks_playback());
}

#[test]
fn the_real_audio_probe_returns() {
    let check = audio::check_default();
    assert!(check.device.as_deref().is_none_or(|d| !d.is_empty()));
}
