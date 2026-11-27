use std::io::{BufRead, BufReader};

use super::*;

#[derive(Clone, Copy, Default)]
pub(super) struct UntrustedUrlState {
    allowed: Option<bool>,
    notice_shown: bool,
}

impl Tail {
    pub(super) fn from_log(path: &Path, offset: u64) -> Self {
        let mut tail = Self {
            offset,
            ..Self::default()
        };
        if offset > 0
            && let Ok(file) = File::open(path)
        {
            let mut log = BufReader::new(file.take(offset));
            let mut line = Vec::new();
            let mut complete_offset = 0;
            while let Ok(read @ 1..) = log.read_until(b'\n', &mut line) {
                if !line.ends_with(b"\n") {
                    tail.offset = complete_offset;
                    break;
                }
                complete_offset += u64::try_from(read).expect("length fits the byte counter");
                if let Some(allowed) = logged_setting(&String::from_utf8_lossy(&line)) {
                    tail.untrusted_urls.allowed = Some(allowed);
                }
                line.clear();
            }
        }
        tail
    }
}

impl VrcLogMonitor {
    pub(super) fn observe_untrusted_urls(&mut self, line: &str, lower: &str) {
        if let Some(allowed) = logged_setting(line) {
            self.untrusted_urls.allowed = Some(allowed);
        } else if is_untrusted_video_block(lower) {
            self.show_untrusted_url_notice();
        }
    }

    pub(super) fn on_unrequested_spotify_error(
        &mut self,
        url: &str,
        lower: &str,
        now: SystemTime,
    ) -> bool {
        if !is_player_error(lower) {
            return false;
        }
        if self.deps.stats.as_ref().is_some_and(|stats| {
            stats.playback.recently_requested(url, now)
                || self
                    .source_of(url)
                    .is_some_and(|source| stats.playback.recently_requested(&source, now))
        }) {
            return false;
        }

        self.show_untrusted_url_notice();
        !self.matches_open_video(url)
    }

    fn show_untrusted_url_notice(&mut self) {
        if self.untrusted_urls.allowed != Some(true) && !self.untrusted_urls.notice_shown {
            self.deps.ux.event(Notice::UntrustedUrlBlocked);
            self.untrusted_urls.notice_shown = true;
        }
    }
}

fn logged_setting(line: &str) -> Option<bool> {
    const SETTING: &str = "Allow untrusted URLs";
    let (label, value) = line.rsplit_once(':')?;
    let value = value.trim();
    let allowed = if value.eq_ignore_ascii_case("true") {
        true
    } else if value.eq_ignore_ascii_case("false") {
        false
    } else {
        return None;
    };
    let label = label.trim_end();
    let start = label.len().checked_sub(SETTING.len())?;
    label
        .get(start..)?
        .eq_ignore_ascii_case(SETTING)
        .then_some(allowed)
}

fn is_untrusted_video_block(lower: &str) -> bool {
    let access_denied = lower.contains("accessdenied") && has_word(lower, "accessdenied");
    let mentions_untrusted_url =
        lower.contains("untrusted url") || lower.contains("not allowed untrusted");
    if !access_denied && !mentions_untrusted_url {
        return false;
    }
    let video_player = [
        "[video playback]",
        "video player",
        "[avprovideo]",
        "[videotxl",
        "[protv",
        "usharpvideo",
    ]
    .iter()
    .any(|player| lower.contains(player));
    if !video_player && !is_player_error(lower) {
        return false;
    }
    access_denied
        || (lower.contains("[avprovideo] error:") && lower.contains("loading failed"))
        || [
            "block",
            "not allowed",
            "not allow",
            "denied",
            "reject",
            "disabled",
            "cannot",
            "can't",
            "disallow",
            "untrusted url:",
        ]
        .iter()
        .any(|denial| lower.contains(denial))
}

fn is_player_error(lower: &str) -> bool {
    lower.contains("error code: player error") || has_word(lower, "playererror")
}

fn has_word(lower: &str, expected: &str) -> bool {
    lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|word| word == expected)
}
