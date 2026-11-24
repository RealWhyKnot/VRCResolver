use std::fs;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use vrcr_watchdog::settings::Store;

fn store(dir: &TempDir) -> (Store, Arc<Mutex<Vec<String>>>) {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&warnings);
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
fn old_settings_keep_the_beta_choice() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("settings.json");
    let previous = r#"{"schema_version":1,"terminal":{"status_line":false,"animations":false},"relay":{"https":"off"},"maintenance":{"update_check":false,"codec_auto_install":false,"include_prereleases":true},"playback":{"high_quality":true}}"#;
    fs::write(&path, previous).unwrap_or_else(|e| panic!("{e}"));
    let (preferences, warnings) = store(&dir);
    assert!(preferences.include_prereleases());
    assert_eq!(fs::read_to_string(&path).unwrap_or_default(), previous);
    assert_eq!(warnings.lock().unwrap_or_else(|e| e.into_inner()).len(), 0);
}

#[test]
fn channel_changes_rewrite_only_the_remaining_preference() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (preferences, _) = store(&dir);
    assert!(!preferences.include_prereleases());
    preferences
        .set_include_prereleases(true)
        .unwrap_or_else(|e| panic!("{e}"));
    let (reopened, _) = store(&dir);
    assert!(reopened.include_prereleases());
    assert_eq!(
        fs::read_to_string(dir.path().join("settings.json")).unwrap_or_default(),
        r#"{"schema_version":1,"maintenance":{"include_prereleases":true}}"#
    );
    reopened
        .set_include_prereleases(false)
        .unwrap_or_else(|e| panic!("{e}"));
    let (reopened, _) = store(&dir);
    assert!(!reopened.include_prereleases());
}

#[test]
fn invalid_preferences_fall_back_to_stable() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(dir.path().join("settings.json"), "not json").unwrap_or_else(|e| panic!("{e}"));
    let (preferences, warnings) = store(&dir);
    assert!(!preferences.include_prereleases());
    assert_eq!(names(&dir).len(), 1);
    assert!(names(&dir)[0].starts_with("settings.json.invalid."));
    assert_eq!(warnings.lock().unwrap_or_else(|e| e.into_inner()).len(), 1);
}

#[test]
fn failed_channel_write_preserves_the_previous_choice() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (preferences, _) = store(&dir);
    preferences
        .set_include_prereleases(true)
        .unwrap_or_else(|e| panic!("{e}"));
    fs::create_dir(dir.path().join("settings.json.new")).unwrap_or_else(|e| panic!("{e}"));
    assert!(preferences.set_include_prereleases(false).is_err());
    assert!(preferences.include_prereleases());
    let (reopened, _) = store(&dir);
    assert!(reopened.include_prereleases());
}

#[test]
fn skipped_version_round_trips_with_the_channel() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (preferences, _) = store(&dir);
    assert_eq!(preferences.skipped_version(), None);
    preferences
        .set_include_prereleases(true)
        .unwrap_or_else(|e| panic!("{e}"));
    preferences
        .set_skipped_version(Some("  2026.11.24.0 \n"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        fs::read_to_string(dir.path().join("settings.json")).unwrap_or_default(),
        r#"{"schema_version":1,"maintenance":{"include_prereleases":true,"skipped_version":"2026.11.24.0"}}"#
    );
    let (reopened, warnings) = store(&dir);
    assert!(reopened.include_prereleases());
    assert_eq!(reopened.skipped_version().as_deref(), Some("2026.11.24.0"));
    assert_eq!(warnings.lock().unwrap_or_else(|e| e.into_inner()).len(), 0);
}

#[test]
fn blank_skipped_version_clears_it() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (preferences, _) = store(&dir);
    preferences
        .set_skipped_version(Some("2026.11.24.0"))
        .unwrap_or_else(|e| panic!("{e}"));
    preferences
        .set_skipped_version(Some("   "))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(preferences.skipped_version(), None);
    assert_eq!(
        fs::read_to_string(dir.path().join("settings.json")).unwrap_or_default(),
        r#"{"schema_version":1,"maintenance":{"include_prereleases":false}}"#
    );
}

#[test]
fn switching_channels_clears_the_skip_but_resaving_keeps_it() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    fs::write(
        dir.path().join("settings.json"),
        r#"{"schema_version":1,"maintenance":{"include_prereleases":false,"skipped_version":"2026.11.24.0"}}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let (preferences, _) = store(&dir);
    preferences
        .set_include_prereleases(false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        preferences.skipped_version().as_deref(),
        Some("2026.11.24.0")
    );
    preferences
        .set_include_prereleases(true)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(preferences.skipped_version(), None);
    let (reopened, _) = store(&dir);
    assert!(reopened.include_prereleases());
    assert_eq!(reopened.skipped_version(), None);
}
