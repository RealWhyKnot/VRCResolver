use std::path::PathBuf;

pub const MANIFEST_REL: &str = "data/release-manifest.tsv";

pub fn normalize_rel(path: &str) -> Option<PathBuf> {
    let cleaned = path.trim().replace('\\', "/");
    if cleaned.is_empty() || cleaned.starts_with('/') || cleaned.contains(':') {
        return None;
    }
    let parts: Vec<&str> = cleaned.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.iter().any(|part| !is_safe_component(part)) {
        return None;
    }
    Some(parts.iter().collect())
}

fn is_safe_component(part: &str) -> bool {
    if part.ends_with(['.', ' '])
        || part
            .chars()
            .any(|character| character.is_control() || "<>\"|?*".contains(character))
    {
        return false;
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return false;
    }
    let port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    !port.is_some_and(|number| {
        matches!(
            number,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{b9}" | "\u{b2}" | "\u{b3}"
        )
    })
}

pub fn manifest_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(
            |line| match line.splitn(3, '\t').collect::<Vec<_>>().as_slice() {
                [_, _, rel] => normalize_rel(rel),
                _ => None,
            },
        )
        .collect()
}
