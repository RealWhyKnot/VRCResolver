use std::time::SystemTime;

use vrcr_shared::console::Color;

use super::text::{
    ANIMATION_WINDOW, Frame, Glyphs, RECENT_WINDOW, Run, activity_glyph, clen, glyphs, pulse_color,
    sparkline, trim_left, trim_right,
};
use crate::display::{format_bytes, format_bytes_per_second};
use crate::stats::{Activity, Bandwidth};

pub const PROMPT: &str = "vrcr> ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineInput<'a> {
    pub mesh_connected: bool,
    pub spinner: usize,
    pub width: usize,
    pub input: &'a str,
    pub status_line: bool,
    pub animations: bool,
    pub unicode: bool,
    pub ghost: &'a str,
    pub cursor: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flags {
    relay_animating: bool,
    upstream_animating: bool,
    relay_recent: bool,
    upstream_recent: bool,
}

impl Flags {
    fn of(activity: &Activity, now: SystemTime) -> Flags {
        Flags {
            relay_animating: activity.relay_active(now, ANIMATION_WINDOW),
            upstream_animating: activity.upstream_active(now, ANIMATION_WINDOW),
            relay_recent: activity.relay_active(now, RECENT_WINDOW),
            upstream_recent: activity.upstream_active(now, RECENT_WINDOW),
        }
    }
}

pub fn normalize_width(width: usize) -> usize {
    if width == 0 { 119 } else { width.clamp(20, 180) }
}

pub fn format(activity: &Activity, bandwidth: &Bandwidth, now: SystemTime, line: &LineInput<'_>) -> Frame {
    let width = normalize_width(line.width);
    if !line.status_line {
        return prompt_only(line.input, line.ghost, line.cursor, width);
    }
    let g = glyphs(line.unicode);
    let flags = Flags::of(activity, now);
    let status = status_runs(activity, bandwidth, line, g, flags, width >= 100, width >= 116);
    let status_len: usize = status.iter().map(|r| clen(&r.text)).sum();

    let mut runs = status;
    runs.push(Run::new("  ", Color::DarkGray));
    runs.push(Run::new(PROMPT, Color::White));
    if !line.input.is_empty() {
        runs.push(Run::new(line.input, Color::Gray));
    }
    if !line.ghost.is_empty() {
        runs.push(Run::new(line.ghost, Color::DarkGray));
    }
    let caret = caret_column(status_len + 2 + PROMPT.len(), line.input, line.cursor);
    let full = Frame::from_runs(runs).with_cursor(caret);
    if full.width() <= width {
        return full;
    }

    let narrow = status_runs(activity, bandwidth, line, g, flags, false, false);
    let status_text: String = narrow.iter().map(|r| r.text.as_str()).collect();
    let fitted = fit(&status_text, line.input, width);
    let cursor = clen(&fitted);
    Frame::plain(fitted, Color::DarkGray).with_cursor(Some(cursor))
}

fn caret_column(input_start: usize, input: &str, cursor: Option<usize>) -> Option<usize> {
    cursor.map(|c| input_start + c.min(clen(input)))
}

fn status_runs(
    activity: &Activity,
    bandwidth: &Bandwidth,
    line: &LineInput<'_>,
    g: &Glyphs,
    f: Flags,
    totals: bool,
    spark: bool,
) -> Vec<Run> {
    let spin = line.spinner;
    let anim = line.animations;
    let mut runs = Vec::with_capacity(24);
    let gap = || Run::new(" ", Color::DarkGray);

    runs.push(Run::new("VRChat", Color::Gray));
    let relay_glyph = activity_glyph(g, f.relay_animating, spin, anim);
    if !relay_glyph.is_empty() {
        runs.push(gap());
        runs.push(Run::new(relay_glyph, pulse_color(spin, 0, anim)));
    }
    runs.push(gap());
    if f.relay_recent {
        runs.push(Run::new(
            "serving",
            pulse_color(spin, 1, f.relay_animating && anim),
        ));
        runs.push(gap());
        runs.push(Run::new(format_bytes_per_second(bandwidth.current), Color::White));
        runs.push(Run::new(" now", Color::DarkGray));
    } else {
        runs.push(Run::new("waiting", Color::DarkGray));
        runs.push(gap());
        runs.push(Run::new(format_bytes(activity.relay_bytes), Color::Gray));
        runs.push(Run::new(" served", Color::DarkGray));
    }
    if totals && f.relay_recent {
        runs.push(gap());
        runs.push(Run::new(
            format!("{} served", format_bytes(activity.relay_bytes)),
            Color::Gray,
        ));
    }
    if spark && bandwidth.has_traffic() {
        let s = sparkline(&bandwidth.history, 8, g);
        if !s.is_empty() {
            runs.push(gap());
            runs.push(Run::new(s, Color::DarkCyan));
        }
    }

    runs.push(Run::new("  resolver", Color::Gray));
    let upstream_glyph = activity_glyph(g, f.upstream_animating, spin.wrapping_add(1), anim);
    if !upstream_glyph.is_empty() {
        runs.push(gap());
        runs.push(Run::new(upstream_glyph, pulse_color(spin, 2, anim)));
    }
    runs.push(gap());
    if f.upstream_recent {
        runs.push(Run::new(
            "pulling",
            pulse_color(spin, 3, f.upstream_animating && anim),
        ));
        if totals && activity.upstream_relay_bytes > 0 {
            runs.push(gap());
            runs.push(Run::new(
                format!("{} received", format_bytes(activity.upstream_relay_bytes)),
                Color::Gray,
            ));
        }
    } else {
        runs.push(Run::new("idle", Color::DarkGray));
    }

    runs.push(gap());
    if line.mesh_connected {
        runs.push(Run::new("online", Color::Green));
    } else {
        runs.push(Run::new("reconnecting", Color::Yellow));
    }
    runs
}

fn prompt_only(input: &str, ghost: &str, cursor: Option<usize>, width: usize) -> Frame {
    let line = fit("", input, width);
    let caret = caret_column(PROMPT.len(), input, cursor);
    if !ghost.is_empty() && clen(&line) + clen(ghost) <= width {
        return Frame::from_runs(vec![
            Run::new(line, Color::Gray),
            Run::new(ghost, Color::DarkGray),
        ])
        .with_cursor(caret);
    }
    Frame::plain(line, Color::Gray).with_cursor(caret)
}

fn join(status: &str, input: &str) -> String {
    if status.is_empty() {
        format!("{PROMPT}{input}")
    } else {
        format!("{status}  {PROMPT}{input}")
    }
}

fn fit(status: &str, input: &str, width: usize) -> String {
    let separator = if status.is_empty() { 0 } else { 2 };
    let room = width.saturating_sub(PROMPT.len());
    let input_budget = width
        .saturating_sub(clen(status) + separator + PROMPT.len())
        .max(clen(input).min(room / 2));
    let shown = trim_left(input, input_budget);
    let line = join(status, &shown);
    if clen(&line) <= width {
        return line;
    }
    let status_budget = width.saturating_sub(2 + PROMPT.len() + clen(&shown));
    let status = trim_right(status, status_budget);
    let line = join(&status, &shown);
    if clen(&line) <= width {
        return line;
    }
    trim_left(&format!("{PROMPT}{input}"), width)
}

#[derive(Default)]
pub struct FrameGate {
    key: Option<GateKey>,
    frame: Option<Frame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GateKey {
    activity: Activity,
    current: u64,
    history: Vec<u64>,
    flags: Flags,
    spinner: usize,
    mesh: bool,
    width: usize,
    input: String,
    ghost: String,
    cursor: Option<usize>,
    status_line: bool,
    animations: bool,
    unicode: bool,
}

impl FrameGate {
    pub fn last_width(&self) -> Option<usize> {
        self.key.as_ref().map(|k| k.width)
    }

    pub fn frame(
        &mut self,
        activity: &Activity,
        bandwidth: &Bandwidth,
        now: SystemTime,
        line: &LineInput<'_>,
    ) -> Frame {
        let flags = Flags::of(activity, now);
        let spinner = if line.animations && (flags.relay_animating || flags.upstream_animating) {
            line.spinner
        } else {
            0
        };
        let key = GateKey {
            activity: *activity,
            current: bandwidth.current,
            history: bandwidth.history.clone(),
            flags,
            spinner,
            mesh: line.mesh_connected,
            width: line.width,
            input: line.input.to_owned(),
            ghost: line.ghost.to_owned(),
            cursor: line.cursor,
            status_line: line.status_line,
            animations: line.animations,
            unicode: line.unicode,
        };
        if self.key.as_ref() == Some(&key)
            && let Some(frame) = &self.frame
        {
            return frame.clone();
        }
        let frame = format(activity, bandwidth, now, line);
        self.key = Some(key);
        self.frame = Some(frame.clone());
        frame
    }
}
