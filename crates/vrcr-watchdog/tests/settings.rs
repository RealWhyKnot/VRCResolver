use std::fs;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use vrcr_watchdog::settings::{self, AppSettings, HTTPS_AUTO, HTTPS_OFF, Store};

fn store(dir: &TempDir) -> (Store, Arc<Mutex<Vec<String>>>) {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = warnings.clone();
    let s = Store::new(
        dir.path().join("settings.json"),
        Box::new(move |w| {
            if let Ok(mut v) = sink.lock() {
                v.push(w.to_owned());
            }
        }),
    );
    (s, warnings)
}

fn names(dir: &TempDir) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir.path())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn defaults_enable_terminal_and_maintenance_settings() {
    let s = AppSettings::default().normalized();
    assert!(s.terminal.status_line);
    assert!(s.terminal.animations);
    assert!(s.maintenance.update_check);
    assert!(s.maintenance.codec_auto_install);
    assert!(!s.maintenance.include_prereleases);
    assert!(!s.playback.high_quality);
    assert_eq!(s.relay.https, HTTPS_AUTO);
}

#[test]
fn include_prereleases_is_exposed_as_a_toggle() {
    let setting = settings::find("include-prereleases").unwrap_or_else(|| panic!("missing"));
    let mut s = AppSettings::default();
    assert_eq!(setting.try_set(&mut s, "on"), Ok(()));
    assert!(s.maintenance.include_prereleases);
    assert_eq!(setting.try_set(&mut s, "off"), Ok(()));
    assert!(!s.maintenance.include_prereleases);
}

#[test]
fn helper_settings_are_no_longer_exposed() {
    for key in [
        "sharing",
        "gpu-sharing",
        "gpu-limit",
        "gpu",
        "gpu-percent",
        "upload-limit",
        "allow-on-battery",
        "encoding-quality",
    ] {
        assert!(settings::find(key).is_none(), "{key}");
    }
}

#[test]
fn every_setting_accepts_plain_toggles_and_resets_to_its_default() {
    for setting in &settings::ALL {
        let mut s = AppSettings::default();
        let default = setting.get(&s);
        assert_eq!(setting.try_set(&mut s, "off"), Ok(()), "{}", setting.key);
        assert_eq!(setting.get(&s), "off");
        assert_eq!(setting.try_set(&mut s, " ENABLED "), Ok(()));
        assert_eq!(setting.get(&s), "on");
        assert_eq!(setting.try_set(&mut s, "maybe"), Err("expected on or off"));
        setting.reset(&mut s);
        assert_eq!(setting.get(&s), default, "{}", setting.key);
    }
}

#[test]
fn aliases_resolve_case_insensitively() {
    assert_eq!(settings::find("HQ").map(|s| s.key), Some("high-quality"));
    assert_eq!(
        settings::find(" relay.https ").map(|s| s.key),
        Some("secure-local-video")
    );
    assert_eq!(settings::find("beta").map(|s| s.key), Some("include-prereleases"));
    assert_eq!(
        settings::find("codec-install").map(|s| s.key),
        Some("video-support-updates")
    );
}

#[test]
fn secure_local_video_maps_onto_the_https_mode() {
    let setting = settings::find("secure-local-video").unwrap_or_else(|| panic!("missing"));
    let mut s = AppSettings::default();
    assert_eq!(setting.try_set(&mut s, "no"), Ok(()));
    assert_eq!(s.relay.https, HTTPS_OFF);
    assert!(setting.restart_required);
}

#[test]
fn https_values_normalise_like_the_old_client() {
    for (raw, want) in [
        ("auto", HTTPS_AUTO),
        ("on", HTTPS_AUTO),
        ("Enabled", HTTPS_AUTO),
        ("OFF", HTTPS_OFF),
        ("disabled", HTTPS_OFF),
        ("false", HTTPS_OFF),
        (" no ", HTTPS_OFF),
        ("", HTTPS_AUTO),
        ("garbage", HTTPS_AUTO),
    ] {
        assert_eq!(settings::normalize_https(raw), want, "{raw:?}");
    }
}

#[test]
fn a_file_from_the_old_client_loads_with_missing_and_null_sections_repaired() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        dir.path().join("settings.json"),
        r#"{"schema_version":7,"terminal":null,"relay":{"https":"disabled"},"maintenance":{"include_prereleases":true},"extra":1}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let (store, warnings) = store(&dir);

    let s = store.snapshot();

    assert_eq!(s.schema_version, 1);
    assert!(s.terminal.status_line && s.terminal.animations);
    assert_eq!(s.relay.https, HTTPS_OFF);
    assert!(s.maintenance.include_prereleases);
    assert!(s.maintenance.update_check);
    assert!(warnings.lock().map(|w| w.is_empty()).unwrap_or(false));
}

#[test]
fn update_saves_compact_json_with_the_old_key_names() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (store, _) = store(&dir);

    let next = store.update(|s| s.playback.high_quality = true);

    assert!(next.playback.high_quality);
    let text = fs::read_to_string(store.path()).unwrap_or_default();
    assert_eq!(
        text,
        r#"{"schema_version":1,"terminal":{"status_line":true,"animations":true},"relay":{"https":"auto"},"maintenance":{"update_check":true,"codec_auto_install":true,"include_prereleases":false},"playback":{"high_quality":true}}"#
    );
    assert_eq!(names(&dir), ["settings.json"]);
    let (reloaded, _) = self::store(&dir);
    assert!(reloaded.snapshot().playback.high_quality);
}

#[test]
fn an_invalid_file_is_quarantined_and_defaults_are_used() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("settings.json"), "{not json").unwrap_or_else(|e| panic!("{e}"));
    let (store, warnings) = store(&dir);

    assert_eq!(store.snapshot(), AppSettings::default());

    let files = names(&dir);
    assert_eq!(files.len(), 1);
    assert!(
        files[0].starts_with("settings.json.invalid.") && files[0].ends_with(".bak"),
        "{files:?}"
    );
    let w = warnings.lock().map(|w| w.clone()).unwrap_or_default();
    assert_eq!(w.len(), 1);
    assert!(w[0].starts_with("ignored invalid settings file; moved to settings.json.invalid."));
}

#[test]
fn an_oversized_file_is_quarantined() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("settings.json"), vec![b' '; 128 * 1024 + 1]).unwrap_or_else(|e| panic!("{e}"));
    let (store, _) = store(&dir);
    assert_eq!(store.snapshot(), AppSettings::default());
    assert!(names(&dir)[0].starts_with("settings.json.too-large."));
}

#[test]
fn a_json_null_file_loads_defaults_without_quarantine() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("settings.json"), "null").unwrap_or_else(|e| panic!("{e}"));
    let (store, _) = store(&dir);
    assert_eq!(store.snapshot(), AppSettings::default());
    assert_eq!(names(&dir), ["settings.json"]);
}
