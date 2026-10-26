use std::path::PathBuf;

pub const MANIFEST_REL: &str = "data/release-manifest.tsv";

pub fn normalize_rel(path: &str) -> Option<PathBuf> {
    let cleaned = path.trim().replace('\\', "/");
    if cleaned.is_empty() || cleaned.starts_with('/') || cleaned.contains(':') {
        return None;
    }
    let parts: Vec<&str> = cleaned.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.iter().any(|p| *p == "." || *p == "..") {
        return None;
    }
    Some(parts.iter().collect())
}

pub fn manifest_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| match line.splitn(3, '\t').collect::<Vec<_>>().as_slice() {
            [_, _, rel] => normalize_rel(rel),
            _ => None,
        })
        .collect()
}
