use std::fs;
use std::io;
use std::path::Path;

pub const TARGET: u32 = 16;
pub const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
pub const VALUE_NAME: &str = "MaxConnectionsPerServer";
pub const MARKER_FILE: &str = "wininet_limit_previous.txt";
const ABSENT: &str = "absent";

pub trait DwordStore {
    fn get(&self) -> Option<u32>;
    fn set(&mut self, value: u32) -> io::Result<()>;
    fn delete(&mut self) -> io::Result<()>;
}

pub fn decide_raise(current: Option<u32>) -> Option<u32> {
    match current {
        Some(v) if v >= TARGET => None,
        _ => Some(TARGET),
    }
}

pub fn ensure(store: &mut dyn DwordStore, marker: &Path, log: &mut dyn FnMut(String)) -> io::Result<()> {
    let current = store.get();
    let Some(target) = decide_raise(current) else {
        return Ok(());
    };
    if !marker.exists() {
        if let Some(dir) = marker.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(
            marker,
            current.map_or_else(|| ABSENT.to_owned(), |v| v.to_string()),
        )?;
    }
    store.set(target)?;
    log(format!(
        "raised WinINet connections per server from {} to {target}; VRChat picks this up the next time it starts",
        current.map_or_else(|| "default 2".to_owned(), |v| v.to_string())
    ));
    Ok(())
}

pub fn restore(
    store: Option<&mut dyn DwordStore>,
    marker: &Path,
    log: &mut dyn FnMut(String),
) -> io::Result<()> {
    let Some(store) = store else {
        let _ = fs::remove_file(marker);
        return Ok(());
    };
    let Ok(previous) = fs::read_to_string(marker) else {
        return Ok(());
    };
    if store.get() == Some(TARGET) {
        match previous.trim().parse::<u32>() {
            Ok(v) => {
                store.set(v)?;
                log(format!("restored WinINet connections per server to {v}"));
            }
            Err(_) => {
                store.delete()?;
                log("removed WinINet connections per server override".to_owned());
            }
        }
    }
    let _ = fs::remove_file(marker);
    Ok(())
}
