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
    pub aliases: &'a [String],
}

fn title_frame(glyphs: &Glyphs, title: &str) -> Frame {
    Frame::from_runs(vec![
        Run::new(format!("{} ", glyphs.bullet), Color::DarkGray),
        Run::new(title, Color::White),
    ])
}

pub fn key_value(title: &str, rows: &[(String, String)], width: usize, glyphs: &Glyphs) -> Vec<Frame> {
    let width = width.max(24);
    let left_width = rows
        .iter()
        .map(|(l, _)| clen(l))
        .max()
        .map_or(0, |m| m.min(24).min((width / 3).max(8)));
    let right_width = width.saturating_sub(left_width + 7).max(8);

    let mut frames = vec![title_frame(glyphs, title)];
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
                Run::new(left_text, if i == 0 { Color::White } else { Color::DarkGray }),
                Run::new("  ", Color::DarkGray),
                Run::new(right_text, Color::Gray),
            ]));
        }
    }
    frames
}

pub fn table(title: &str, rows: &[(String, String, String)], width: usize, glyphs: &Glyphs) -> Vec<Frame> {
    if rows.is_empty() {
        return key_value(title, &[], width, glyphs);
    }
    let width = width.max(24);
    let longest =
        |f: fn(&(String, String, String)) -> &str| rows.iter().map(|r| clen(f(r))).max().unwrap_or(0);
    let name_width = longest(|r| &r.0).min(22).min((width / 4).max(8));
    let value_width = longest(|r| &r.1).min(18).min((width / 5).max(8));
    let description_width = width as i64 - name_width as i64 - value_width as i64 - 10;
    if description_width < 16 {
        let flat: Vec<(String, String)> = rows
            .iter()
            .map(|(n, v, d)| (n.clone(), format!("{v} - {d}")))
            .collect();
        return key_value(title, &flat, width, glyphs);
    }
    let description_width = usize::try_from(description_width).unwrap_or(16);

    let mut frames = vec![
        title_frame(glyphs, title),
        Frame::from_runs(vec![
            Run::new(format!("  {}", pad_right("setting", name_width)), Color::DarkGray),
            Run::new(
                format!("  {}", pad_right("current", value_width)),
                Color::DarkGray,
            ),
            Run::new("  about", Color::DarkGray),
        ]),
    ];
    for (name, value, description) in rows {
        let descriptions = wrap(description, description_width);
        let count = descriptions.len().max(1);
        for i in 0..count {
            let (n, v) = if i == 0 {
                (
                    pad_right(&trim_right(name, name_width), name_width),
                    pad_right(&trim_right(value, value_width), value_width),
                )
            } else {
                (" ".repeat(name_width), " ".repeat(value_width))
            };
            frames.push(Frame::from_runs(vec![
                Run::new(
                    format!("  {n}"),
                    if i == 0 { Color::White } else { Color::DarkGray },
                ),
                Run::new("  ", Color::DarkGray),
                Run::new(v, value_color(value)),
                Run::new("  ", Color::DarkGray),
                Run::new(descriptions.get(i).cloned().unwrap_or_default(), Color::Gray),
            ]));
        }
    }
    frames
}

pub fn panel(title: &str, rows: &[(String, String, String)], width: usize, glyphs: &Glyphs) -> Vec<Frame> {
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
    let name_width = rows.iter().map(|r| clen(&r.0)).max().unwrap_or(0).clamp(8, 16);
    let state_width = rows.iter().map(|r| clen(&r.1)).max().unwrap_or(0).clamp(8, 18);
    let detail_width = inner.saturating_sub(name_width + state_width + 6).max(8);
    let heading = format!(" {title} ");
    let top_fill = glyphs.horizontal.repeat(inner.saturating_sub(clen(&heading)));

    let mut frames = vec![Frame::from_runs(vec![
        Run::new(glyphs.top_left, Color::DarkGray),
        Run::new(heading, Color::White),
        Run::new(top_fill, Color::DarkGray),
        Run::new(glyphs.top_right, Color::DarkGray),
    ])];
    for (name, state, detail) in rows {
        frames.push(Frame::from_runs(vec![
            Run::new(format!("{} ", glyphs.vertical), Color::DarkGray),
            Run::new(pad_right(&trim_right(name, name_width), name_width), Color::White),
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

pub fn command_palette(commands: &[PaletteCommand<'_>], width: usize, glyphs: &Glyphs) -> Vec<Frame> {
    let width = width.max(24);
    let mut frames = vec![Frame::from_runs(vec![
        Run::new(format!("{} ", glyphs.bullet), Color::DarkGray),
        Run::new("commands", Color::White),
        Run::new("  tab completes a unique match", Color::DarkGray),
    ])];
    let command_width = commands
        .iter()
        .map(|c| clen(c.name) + 1)
        .max()
        .unwrap_or(8)
        .clamp(8, 18);
    let description_width = width.saturating_sub(command_width + 8).max(8);
    for c in commands {
        let alias = if c.aliases.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = c.aliases.iter().map(|a| format!("/{a}")).collect();
            format!("  {}", list.join(", "))
        };
        frames.push(Frame::from_runs(vec![
            Run::new(format!("  {} ", glyphs.detail), Color::DarkGray),
            Run::new(pad_right(&format!("/{}", c.name), command_width), Color::White),
            Run::new("  ", Color::DarkGray),
            Run::new(
                trim_right(&format!("{}{alias}", c.description), description_width),
                Color::Gray,
            ),
        ]));
    }
    frames
}

pub fn completion_palette(items: &[Item], width: usize, glyphs: &Glyphs) -> Vec<Frame> {
    let width = width.max(24);
    let mut frames = vec![title_frame(glyphs, "matches")];
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
            Run::new(trim_right(&item.description, description_width), Color::Gray),
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

fn value_color(value: &str) -> Color {
    let v = value.trim().to_lowercase();
    if v.starts_with("on") {
        Color::Green
    } else if v.starts_with("off") {
        Color::DarkGray
    } else if v.starts_with("automatic") {
        Color::Gray
    } else {
        Color::White
    }
}

pub fn state_color(value: &str) -> Color {
    let v = value.trim().to_lowercase();
    let has = |s: &str| v.contains(s);
    if v == "on" || v == "ready" || v == "eligible" || has("online") || has("serving") || has("pulling") {
        Color::Green
    } else if has("reconnecting") || has("fallback") || has("paused") || has("setup") || has("timeout") {
        Color::Yellow
    } else if has("failed") || has("missing") || has("not eligible") || v == "off" {
        Color::Red
    } else {
        Color::Gray
    }
}
