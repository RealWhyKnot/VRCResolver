use std::time::{Duration, SystemTime};

use vrcr_shared::console::Color;

use crate::settings::AppSettings;
use crate::stats::Activity;

pub const ACTIVE_REFRESH: Duration = Duration::from_millis(125);
pub const IDLE_REFRESH: Duration = Duration::from_secs(1);
pub const ANIMATION_WINDOW: Duration = Duration::from_secs(2);
pub const RECENT_WINDOW: Duration = Duration::from_secs(5);

pub fn fast_refresh(
    activity: &Activity,
    now: SystemTime,
    settings: &AppSettings,
    animations_available: bool,
) -> bool {
    if !settings.terminal.status_line || !settings.terminal.animations || !animations_available {
        return false;
    }
    activity.relay_active(now, ANIMATION_WINDOW) || activity.upstream_active(now, ANIMATION_WINDOW)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub color: Color,
}

impl Run {
    pub fn new(text: impl Into<String>, color: Color) -> Run {
        Run {
            text: text.into(),
            color,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frame {
    pub runs: Vec<Run>,
    pub plain: String,
    pub cursor: Option<usize>,
}

impl Frame {
    pub fn from_runs(runs: Vec<Run>) -> Frame {
        let plain = runs.iter().map(|r| r.text.as_str()).collect();
        Frame {
            runs,
            plain,
            cursor: None,
        }
    }

    pub fn plain(text: impl Into<String>, color: Color) -> Frame {
        Frame::from_runs(vec![Run::new(text, color)])
    }

    pub fn with_cursor(mut self, cursor: Option<usize>) -> Frame {
        self.cursor = cursor;
        self
    }

    pub fn width(&self) -> usize {
        clen(&self.plain)
    }
}

pub fn clen(s: &str) -> usize {
    s.chars().count()
}

pub fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn last_chars(s: &str, n: usize) -> &str {
    let len = clen(s);
    if n >= len {
        return s;
    }
    match s.char_indices().nth(len - n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

pub fn pad_right(s: &str, width: usize) -> String {
    let len = clen(s);
    if len >= width {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() + width - len);
    out.push_str(s);
    out.extend(std::iter::repeat_n(' ', width - len));
    out
}

pub fn pad_left(s: &str, width: usize) -> String {
    let len = clen(s);
    if len >= width {
        return s.to_owned();
    }
    let mut out: String = std::iter::repeat_n(' ', width - len).collect();
    out.push_str(s);
    out
}

pub fn trim_right(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if clen(value) <= max {
        return value.to_owned();
    }
    if max <= 2 {
        return take_chars(value, max).to_owned();
    }
    format!("{}..", take_chars(value, max - 2))
}

pub fn trim_left(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if clen(value) <= max {
        return value.to_owned();
    }
    if max <= 3 {
        return last_chars(value, max).to_owned();
    }
    format!("...{}", last_chars(value, max - 3))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Glyphs {
    pub bullet: &'static str,
    pub detail: &'static str,
    pub horizontal: &'static str,
    pub vertical: &'static str,
    pub top_left: &'static str,
    pub top_right: &'static str,
    pub bottom_left: &'static str,
    pub bottom_right: &'static str,
    pub spinner: [&'static str; 4],
    pub sparkline: [&'static str; 8],
    pub still: &'static str,
}

pub static UNICODE: Glyphs = Glyphs {
    bullet: "\u{2022}",
    detail: "\u{25e6}",
    horizontal: "\u{2500}",
    vertical: "\u{2502}",
    top_left: "\u{250c}",
    top_right: "\u{2510}",
    bottom_left: "\u{2514}",
    bottom_right: "\u{2518}",
    spinner: ["\u{25d0}", "\u{25d3}", "\u{25d1}", "\u{25d2}"],
    sparkline: [
        "\u{2581}", "\u{2582}", "\u{2583}", "\u{2584}", "\u{2585}", "\u{2586}", "\u{2587}", "\u{2588}",
    ],
    still: "\u{25cf}",
};

pub static ASCII: Glyphs = Glyphs {
    bullet: "*",
    detail: "-",
    horizontal: "-",
    vertical: "|",
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    spinner: ["|", "/", "-", "\\"],
    sparkline: ["_", ".", "-", "=", "#", "#", "#", "#"],
    still: "*",
};

pub fn glyphs(unicode: bool) -> &'static Glyphs {
    if unicode { &UNICODE } else { &ASCII }
}

pub fn activity_glyph(glyphs: &Glyphs, active: bool, frame: usize, animations: bool) -> &'static str {
    if !active {
        return "";
    }
    if !animations {
        return glyphs.still;
    }
    glyphs.spinner[frame % glyphs.spinner.len()]
}

pub fn pulse_color(frame: usize, phase: usize, active: bool) -> Color {
    if !active {
        return Color::DarkGray;
    }
    match frame.wrapping_add(phase) & 3 {
        0 => Color::Gray,
        1 => Color::White,
        2 => Color::DarkCyan,
        _ => Color::Green,
    }
}

pub fn sparkline(samples: &[u64], width: usize, glyphs: &Glyphs) -> String {
    if samples.is_empty() || width == 0 {
        return String::new();
    }
    let count = width.min(samples.len());
    let tail = &samples[samples.len() - count..];
    let max = tail.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return String::new();
    }
    let top = (glyphs.sparkline.len() - 1) as u128;
    tail.iter()
        .map(|&v| {
            let index = (u128::from(v) * top / u128::from(max)).min(top);
            glyphs.sparkline[usize::try_from(index).unwrap_or(0)]
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub verb: String,
    pub args: String,
}

impl CommandLine {
    pub fn parse(text: &str) -> CommandLine {
        let text = text.trim();
        match text.find([' ', '\t']) {
            None => CommandLine {
                verb: normalize_verb(text),
                args: String::new(),
            },
            Some(i) => CommandLine {
                verb: normalize_verb(&text[..i]),
                args: text[i + 1..].trim().to_owned(),
            },
        }
    }
}

pub fn normalize_verb(verb: &str) -> String {
    verb.trim().trim_start_matches('/').to_lowercase()
}
