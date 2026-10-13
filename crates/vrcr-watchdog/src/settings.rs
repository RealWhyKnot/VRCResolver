use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Deserializer, Serialize};
use vrcr_shared::logutil::UtcStamp;

pub const HTTPS_AUTO: &str = "auto";
pub const HTTPS_OFF: &str = "off";
const MAX_SETTINGS_BYTES: u64 = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: i32,
    #[serde(deserialize_with = "null_default")]
    pub terminal: TerminalSettings,
    #[serde(deserialize_with = "null_default")]
    pub relay: RelaySettings,
    #[serde(deserialize_with = "null_default")]
    pub maintenance: MaintenanceSettings,
    #[serde(deserialize_with = "null_default")]
    pub playback: PlaybackSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub status_line: bool,
    pub animations: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelaySettings {
    #[serde(deserialize_with = "null_default")]
    pub https: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaintenanceSettings {
    pub update_check: bool,
    pub codec_auto_install: bool,
    pub include_prereleases: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    pub high_quality: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            schema_version: 1,
            terminal: TerminalSettings::default(),
            relay: RelaySettings::default(),
            maintenance: MaintenanceSettings::default(),
            playback: PlaybackSettings::default(),
        }
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        TerminalSettings {
            status_line: true,
            animations: true,
        }
    }
}

impl Default for RelaySettings {
    fn default() -> Self {
        RelaySettings {
            https: HTTPS_AUTO.to_owned(),
        }
    }
}

impl Default for MaintenanceSettings {
    fn default() -> Self {
        MaintenanceSettings {
            update_check: true,
            codec_auto_install: true,
            include_prereleases: false,
        }
    }
}

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

impl AppSettings {
    pub fn normalized(mut self) -> AppSettings {
        self.schema_version = 1;
        self.relay.https = normalize_https(&self.relay.https).to_owned();
        self
    }

    pub fn https_enabled(&self) -> bool {
        self.relay.https != HTTPS_OFF
    }
}

pub fn normalize_https(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "false" | "no" => HTTPS_OFF,
        _ => HTTPS_AUTO,
    }
}

pub struct Store {
    path: PathBuf,
    cached: Mutex<Option<AppSettings>>,
    warn: Box<dyn Fn(&str) + Send + Sync>,
}

impl Store {
    pub fn new(path: PathBuf, warn: Box<dyn Fn(&str) + Send + Sync>) -> Store {
        Store {
            path,
            cached: Mutex::new(None),
            warn,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> AppSettings {
        let mut g = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(|| self.load()).clone()
    }

    pub fn update(&self, mutate: impl FnOnce(&mut AppSettings)) -> AppSettings {
        let mut g = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = g.get_or_insert_with(|| self.load()).clone();
        mutate(&mut next);
        let next = next.normalized();
        self.save(&next);
        *g = Some(next.clone());
        next
    }

    fn load(&self) -> AppSettings {
        let Ok(meta) = fs::metadata(&self.path) else {
            return AppSettings::default();
        };
        if meta.len() > MAX_SETTINGS_BYTES {
            self.quarantine("too-large");
            return AppSettings::default();
        }
        match fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Option<AppSettings>>(&b).ok())
        {
            Some(s) => s.unwrap_or_default().normalized(),
            None => {
                self.quarantine("invalid");
                AppSettings::default()
            }
        }
    }

    fn save(&self, settings: &AppSettings) {
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
                fs::create_dir_all(dir)?;
            }
            let mut tmp = self.path.as_os_str().to_owned();
            tmp.push(".new");
            fs::write(&tmp, serde_json::to_vec(settings).map_err(std::io::Error::other)?)?;
            fs::rename(&tmp, &self.path)
        })();
        if let Err(e) = result {
            (self.warn)(&format!("settings save failed: {e}"));
        }
    }

    fn quarantine(&self, reason: &str) {
        if !self.path.exists() {
            return;
        }
        let s = UtcStamp::at(SystemTime::now());
        let stamp = format!(
            "{:04}{:02}{:02}{:02}{:02}{:02}",
            s.year, s.month, s.day, s.hour, s.minute, s.second
        );
        let mut dst = self.path.as_os_str().to_owned();
        dst.push(format!(".{reason}.{stamp}.bak"));
        let dst = PathBuf::from(dst);
        if fs::rename(&self.path, &dst).is_ok() {
            let name = dst
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (self.warn)(&format!("ignored invalid settings file; moved to {name}"));
        }
    }
}

pub struct Setting {
    pub key: &'static str,
    pub description: &'static str,
    pub restart_required: bool,
    pub aliases: &'static [&'static str],
    get: fn(&AppSettings) -> bool,
    put: fn(&mut AppSettings, bool),
}

pub const CHOICES: [&str; 2] = ["on", "off"];

impl Setting {
    pub fn choices(&self) -> &'static [&'static str] {
        &CHOICES
    }

    pub fn get(&self, s: &AppSettings) -> &'static str {
        if (self.get)(s) { "on" } else { "off" }
    }

    pub fn try_set(&self, s: &mut AppSettings, value: &str) -> Result<(), &'static str> {
        (self.put)(s, parse_toggle(value)?);
        Ok(())
    }

    pub fn reset(&self, s: &mut AppSettings) {
        (self.put)(s, (self.get)(&AppSettings::default()));
    }
}

pub fn parse_toggle(value: &str) -> Result<bool, &'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" | "enabled" => Ok(true),
        "off" | "false" | "no" | "0" | "disabled" => Ok(false),
        _ => Err("expected on or off"),
    }
}

pub static ALL: [Setting; 7] = [
    Setting {
        key: "status-line",
        description: "Show the live status line at the prompt.",
        restart_required: false,
        aliases: &["status", "terminal.status-line"],
        get: |s| s.terminal.status_line,
        put: |s, v| s.terminal.status_line = v,
    },
    Setting {
        key: "animations",
        description: "Animate the active video traffic indicators.",
        restart_required: false,
        aliases: &["terminal.animations"],
        get: |s| s.terminal.animations,
        put: |s, v| s.terminal.animations = v,
    },
    Setting {
        key: "secure-local-video",
        description: "Use secure local video links when Windows allows it.",
        restart_required: true,
        aliases: &["local-https", "relay.https"],
        get: AppSettings::https_enabled,
        put: |s, v| s.relay.https = if v { HTTPS_AUTO } else { HTTPS_OFF }.to_owned(),
    },
    Setting {
        key: "update-check",
        description: "Check for new VRCResolver versions at startup.",
        restart_required: true,
        aliases: &["updates", "maintenance.update-check"],
        get: |s| s.maintenance.update_check,
        put: |s, v| s.maintenance.update_check = v,
    },
    Setting {
        key: "include-prereleases",
        description: "Offer prereleases when checking for new versions.",
        restart_required: true,
        aliases: &[
            "prereleases",
            "prerelease",
            "beta",
            "maintenance.include-prereleases",
        ],
        get: |s| s.maintenance.include_prereleases,
        put: |s, v| s.maintenance.include_prereleases = v,
    },
    Setting {
        key: "video-support-updates",
        description: "Keep playback support files current.",
        restart_required: true,
        aliases: &["video-updates", "codec-install", "maintenance.codec-install"],
        get: |s| s.maintenance.codec_auto_install,
        put: |s, v| s.maintenance.codec_auto_install = v,
    },
    Setting {
        key: "high-quality",
        description: "Play the best rung a video offers instead of the quality VRChat asked for. Uses more bandwidth, and above 1080p most sources need a VP9 or AV1 decoder.",
        restart_required: false,
        aliases: &["hq", "quality", "playback.high-quality"],
        get: |s| s.playback.high_quality,
        put: |s, v| s.playback.high_quality = v,
    },
];

pub fn find(key: &str) -> Option<&'static Setting> {
    let key = key.trim();
    ALL.iter()
        .find(|s| s.key.eq_ignore_ascii_case(key) || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(key)))
}
