use vrcr_shared::console::Color;

use super::text::{Frame, Glyphs, Run, clen, pad_right, take_chars, trim_right};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub text: String,
    pub description: String,
}

impl Item {
    pub fn new(text: impl Into<String>, description: impl Into<String>) -> Item {
        Item {
            text: text.into(),
            description: description.into(),
        }
    }
}

pub struct PaletteCommand<'a> {
    pub name: &'a str,
    pub description: &'a str,
}

fn title_frame(title: &str) -> Frame {
    Frame::from_runs(vec![Run::new(title, Color::White)])
}

pub fn key_value(
    title: &str,
    rows: &[(String, String)],
    width: usize,
    glyphs: &Glyphs,
) -> Vec<Frame> {
    let width = width.max(24);
    let left_width = rows
        .iter()
        .map(|(l, _)| clen(l))
        .max()
        .map_or(0, |m| m.min(24).min((width / 3).max(8)));
    let right_width = width.saturating_sub(left_width + 7).max(8);

    let mut frames = vec![title_frame(title)];
    for (left, right) in rows {
        let wrapped = wrap(right, right_width);
        let count = wrapped.len().max(1);
        for i in 0..count {
            let left_text = if i == 0 {
                pad_right(&trim_right(left, left_width), left_width)
            } else {
                " ".repeat(left_width)
            };
            let right_text = wrapped.get(i).cloned().unwrap_or_default();
            frames.push(Frame::from_runs(vec![
                Run::new(format!("  {} ", glyphs.detail), Color::DarkGray),
                Run::new(
                    left_text,
                    if i == 0 {
                        Color::White
                    } else {
                        Color::DarkGray
                    },
                ),
                Run::new("  ", Color::DarkGray),
                Run::new(right_text, Color::Gray),
            ]));
        }
    }
    frames
}

pub fn panel(
    title: &str,
    rows: &[(String, String, String)],
    width: usize,
    glyphs: &Glyphs,
) -> Vec<Frame> {
    let width = width.clamp(30, 120);
    if rows.is_empty() {
        return key_value(title, &[], width, glyphs);
    }
    if width < 48 {
        let flat: Vec<(String, String)> = rows
            .iter()
            .map(|(n, s, d)| (n.clone(), format!("{s}  {d}")))
            .collect();
        return key_value(title, &flat, width, glyphs);
    }
    let inner = width - 2;
    let name_width = rows
        .iter()
        .map(|r| clen(&r.0))
        .max()
        .unwrap_or(0)
        .clamp(8, 16);
    let state_width = rows
        .iter()
        .map(|r| clen(&r.1))
        .max()
        .unwrap_or(0)
        .clamp(8, 18);
    let detail_width = inner.saturating_sub(name_width + state_width + 6).max(8);
    let heading = format!(" {title} ");
    let top_fill = glyphs
        .horizontal
        .repeat(inner.saturating_sub(clen(&heading)));

    let mut frames = vec![Frame::from_runs(vec![
        Run::new(glyphs.top_left, Color::DarkGray),
        Run::new(heading, Color::White),
        Run::new(top_fill, Color::DarkGray),
        Run::new(glyphs.top_right, Color::DarkGray),
    ])];
    for (name, state, detail) in rows {
        frames.push(Frame::from_runs(vec![
            Run::new(format!("{} ", glyphs.vertical), Color::DarkGray),
            Run::new(
                pad_right(&trim_right(name, name_width), name_width),
                Color::White,
            ),
            Run::new("  ", Color::DarkGray),
            Run::new(
                pad_right(&trim_right(state, state_width), state_width),
                state_color(state),
            ),
            Run::new("  ", Color::DarkGray),
            Run::new(
                pad_right(&trim_right(detail, detail_width), detail_width),
                Color::Gray,
            ),
            Run::new(format!(" {}", glyphs.vertical), Color::DarkGray),
        ]));
    }
    frames.push(Frame::from_runs(vec![
        Run::new(glyphs.bottom_left, Color::DarkGray),
        Run::new(glyphs.horizontal.repeat(inner), Color::DarkGray),
        Run::new(glyphs.bottom_right, Color::DarkGray),
    ]));
    frames
}

pub fn command_palette(
    title: &str,
    commands: &[PaletteCommand<'_>],
    width: usize,
    glyphs: &Glyphs,
) -> Vec<Frame> {
    let width = width.max(20);
    let mut frames = vec![title_frame(title)];
    let command_width = commands
        .iter()
        .map(|c| clen(c.name))
        .max()
        .unwrap_or(8)
        .clamp(8, 18);
    let column = command_width + 6;
    for c in commands {
        let mut label = vec![
            Run::new(format!("  {} ", glyphs.detail), Color::DarkGray),
            Run::new(c.name, Color::White),
        ];
        let margin = if width.saturating_sub(column) < 16 {
            4
        } else {
            column
        };
        let description = wrap(c.description, width - margin);
        let mut lines = description.into_iter();
        if margin == column
            && let Some(first) = lines.next()
        {
            label.push(Run::new(
                " ".repeat((column - 4).saturating_sub(clen(c.name))),
                Color::Gray,
            ));
            label.push(Run::new(first, Color::Gray));
        }
        frames.push(Frame::from_runs(label));
        frames.extend(
            lines.map(|line| Frame::plain(format!("{}{line}", " ".repeat(margin)), Color::Gray)),
        );
    }
    frames
}

pub fn completion_palette(items: &[Item], width: usize, glyphs: &Glyphs) -> Vec<Frame> {
    let width = width.max(24);
    let mut frames = vec![title_frame("matches")];
    let item_width = items
        .iter()
        .map(|i| clen(&i.text) + 1)
        .max()
        .unwrap_or(8)
        .clamp(8, 24);
    let description_width = width.saturating_sub(item_width + 8).max(8);
    for item in items {
        frames.push(Frame::from_runs(vec![
            Run::new(format!("  {} ", glyphs.detail), Color::DarkGray),
            Run::new(pad_right(&item.text, item_width), Color::White),
            Run::new("  ", Color::DarkGray),
            Run::new(
                trim_right(&item.description, description_width),
                Color::Gray,
            ),
        ]));
    }
    frames
}

pub fn wrap(value: &str, width: usize) -> Vec<String> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    let width = width.max(8);
    let mut lines = Vec::new();
    for paragraph in value.split('\n') {
        let mut remaining: String = paragraph.trim().to_owned();
        while clen(&remaining) > width {
            let head = take_chars(&remaining, width + 1);
            let split = head
                .char_indices()
                .filter(|(_, c)| *c == ' ')
                .map(|(i, _)| i)
                .next_back()
                .filter(|&i| i > 0)
                .unwrap_or_else(|| take_chars(&remaining, width).len());
            lines.push(remaining[..split].trim_end().to_owned());
            remaining = remaining[split..].trim_start().to_owned();
        }
        if !remaining.is_empty() {
            lines.push(remaining);
        }
    }
    lines
}

pub fn state_color(value: &str) -> Color {
    let v = value.trim().to_lowercase();
    let has = |s: &str| v.contains(s);
    if v == "on"
        || v == "ready"
        || v == "eligible"
        || has("online")
        || has("serving")
        || has("pulling")
    {
        Color::Green
    } else if has("reconnecting")
        || has("fallback")
        || has("paused")
        || has("setup")
        || has("timeout")
    {
        Color::Yellow
    } else if has("failed") || has("missing") || has("not eligible") || v == "off" {
        Color::Red
    } else {
        Color::Gray
    }
}
