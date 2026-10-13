use std::path::{Path, PathBuf};

pub const OG_NAME: &str = "yt-dlp-og.exe";
pub const BUNDLED_NAME: &str = "yt-dlp.exe";

pub fn select(
    exe_dir: &Path,
    vrc_tools_dir: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
    is_our_wrapper: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let tools = vrc_tools_dir.filter(|d| !d.as_os_str().is_empty());
    std::iter::once(exe_dir.join(OG_NAME))
        .chain(
            tools
                .into_iter()
                .flat_map(|d| [d.join(OG_NAME), d.join(BUNDLED_NAME)]),
        )
        .find(|c| exists(c) && !is_our_wrapper(c))
}
