use std::fs::File;
use std::io::BufReader;
use std::path::Path;

const MIB: u64 = 1024 * 1024;

pub fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(zip).map_err(|e| format!("could not open {}: {e}", zip.display()))?;
    let mut archive =
        zip::ZipArchive::new(BufReader::new(file)).map_err(|e| format!("release zip is unreadable: {e}"))?;
    archive
        .extract(dest)
        .map_err(|e| format!("could not extract release zip: {e}"))
}

pub fn check_free_space(total: u64, free: Option<u64>, drive: &str) -> Result<(), String> {
    let Some(free) = free else {
        return Ok(());
    };
    let needed = total.saturating_add(total / 2);
    if free < needed {
        return Err(format!(
            "Insufficient free space on {drive} for download (need ~{} MiB, have {} MiB).",
            needed / MIB,
            free / MIB
        ));
    }
    Ok(())
}
