use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const FILE_NAME: &str = "client_id.txt";
pub const MAX_FILE_BYTES: u64 = 256;
const READ_RETRIES: u32 = 3;
const READ_RETRY_DELAY: Duration = Duration::from_millis(50);

pub fn fresh_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn parse_simple_guid(text: &str) -> Option<String> {
    (text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())).then(|| text.to_ascii_lowercase())
}

pub fn load_or_create(path: &Path) -> String {
    for attempt in 0..=READ_RETRIES {
        match read_existing(path) {
            Ok(Some(id)) => return id,
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(_) if attempt < READ_RETRIES => std::thread::sleep(READ_RETRY_DELAY),
            Err(_) => return fresh_id(),
        }
    }
    let fresh = fresh_id();
    let tmp = sibling(path, ".new");
    let written = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&tmp, &fresh))
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    fresh
}

fn read_existing(path: &Path) -> io::Result<Option<String>> {
    let meta = fs::metadata(path)?;
    if meta.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    Ok(parse_simple_guid(String::from_utf8_lossy(&bytes).trim()))
}

pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}
