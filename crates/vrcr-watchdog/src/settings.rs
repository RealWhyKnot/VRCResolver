use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Deserializer, Serialize};
use vrcr_shared::logutil::UtcStamp;

const MAX_SETTINGS_BYTES: u64 = 128 * 1024;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
struct Maintenance {
    include_prereleases: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    schema_version: i32,
    #[serde(deserialize_with = "null_default")]
    maintenance: Maintenance,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            maintenance: Maintenance::default(),
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

pub struct Store {
    path: PathBuf,
    cached: Mutex<Option<bool>>,
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

    pub fn include_prereleases(&self) -> bool {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        *cached.get_or_insert_with(|| self.load())
    }

    pub fn set_include_prereleases(&self, include_prereleases: bool) -> std::io::Result<()> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        self.save(include_prereleases)?;
        *cached = Some(include_prereleases);
        drop(cached);
        Ok(())
    }

    fn load(&self) -> bool {
        let Ok(meta) = fs::metadata(&self.path) else {
            return false;
        };
        if meta.len() > MAX_SETTINGS_BYTES {
            self.quarantine("too-large");
            return false;
        }
        match fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Option<Preferences>>(&b).ok())
        {
            Some(s) => s.unwrap_or_default().maintenance.include_prereleases,
            None => {
                self.quarantine("invalid");
                false
            }
        }
    }

    fn save(&self, include_prereleases: bool) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let mut tmp = self.path.as_os_str().to_owned();
        tmp.push(".new");
        let preferences = Preferences {
            maintenance: Maintenance {
                include_prereleases,
            },
            ..Preferences::default()
        };
        fs::write(
            &tmp,
            serde_json::to_vec(&preferences).map_err(std::io::Error::other)?,
        )?;
        fs::rename(tmp, &self.path)
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
