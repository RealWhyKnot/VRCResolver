mod replies;

use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use vrcr_shared::console::{Color, Component, Overlay, OverlayGuard, Ux};

use super::blocks::{self, Item, PaletteCommand};
use super::commands::Command;
use super::input::InputBuffer;
use super::overlay::{OverlayLine, park_caret, write_frame_line};
use super::status::{FrameGate, LineInput, PROMPT};
use super::text::{Frame, Glyphs, Run, fast_refresh, glyphs, sparkline};
use crate::display::{format_bytes, format_bytes_per_second};
use crate::maintenance::update::CheckStart;
use crate::settings::Store;
use crate::stats::{Bandwidth, Stats};

pub const BANDWIDTH_SECONDS: usize = 24;
pub const DEFAULT_WIDTH: usize = 119;

pub type Flag = Arc<dyn Fn() -> bool + Send + Sync>;
pub type Action = Arc<dyn Fn() + Send + Sync>;
pub type UpdateCheck = Arc<dyn Fn() -> CheckStart + Send + Sync>;

#[derive(Clone)]
pub struct Services {
    pub ux: Ux,
    pub stats: Arc<Stats>,
    pub settings: Arc<Store>,
    pub mesh_connected: Flag,
    pub repair: Action,
    pub update_check: UpdateCheck,
    pub width: fn() -> Option<usize>,
    pub animations_available: bool,
    pub now: fn() -> SystemTime,
}

#[derive(Default)]
pub struct InputState {
    pub buffer: InputBuffer,
    pub ghost: String,
}

struct Inner {
    services: Services,
    input: Mutex<InputState>,
    spinner: AtomicUsize,
}

#[derive(Clone)]
pub struct Renderer {
    inner: Arc<Inner>,
}

impl Renderer {
    pub fn new(services: Services, input: InputBuffer) -> Renderer {
        Renderer {
            inner: Arc::new(Inner {
                services,
                input: Mutex::new(InputState {
                    buffer: input,
                    ghost: String::new(),
                }),
                spinner: AtomicUsize::new(0),
            }),
        }
    }

    pub fn input(&self) -> MutexGuard<'_, InputState> {
        self.inner.input.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn tick(&self) {
        self.inner.spinner.fetch_add(1, Ordering::Relaxed);
    }

    pub fn settings(&self) -> &Store {
        &self.inner.services.settings
    }

    pub fn repair(&self) {
        (self.inner.services.repair)();
    }

    pub fn start_update_check(&self) -> CheckStart {
        (self.inner.services.update_check)()
    }

    pub fn attach(&self) -> OverlayGuard {
        self.inner.services.ux.use_overlay(Box::new(StatusOverlay {
            inner: std::sync::Arc::clone(&self.inner),
            line: OverlayLine::default(),
            gate: FrameGate::default(),
        }))
    }

    pub fn render_overlay(&self) {
        self.inner.services.ux.refresh_overlay();
    }

    pub fn fast_refresh(&self) -> bool {
        let s = &self.inner.services;
        fast_refresh(&s.stats.activity(), (s.now)(), s.animations_available)
    }

    pub fn echo_command(&self, command: &str) {
        let frame = prompt_frame(command);
        self.write_frames(std::slice::from_ref(&frame));
    }

    pub fn clear_screen(&self) {
        self.inner.services.ux.with_overlay_cleared(|out| {
            let _ = out.write_all(b"\x1b[2J\x1b[3J\x1b[H");
        });
    }

    pub fn render_help(&self, commands: &[Command]) {
        let palette: Vec<PaletteCommand<'_>> = commands
            .iter()
            .map(|c| PaletteCommand {
                name: &c.name,
                description: &c.description,
            })
            .collect();
        self.write_frames(&blocks::command_palette(
            "commands",
            &palette,
            self.usable_width(),
            self.glyphs(),
        ));
        self.info("Press Tab to complete a command or type help <command> for details.");
    }

    pub fn render_command_help(&self, command: &Command) {
        let rows = vec![
            ("command".to_owned(), command.name.clone()),
            ("about".to_owned(), command.description.clone()),
        ];
        self.write_block("help", &rows);
    }

    pub fn render_unknown_command(&self, verb: &str, nearest: &[Item]) {
        self.warn(&format!("unknown command: {verb}"));
        if nearest.is_empty() {
            self.info("type help for the full list.");
            return;
        }
        self.write_frames(&blocks::completion_palette(
            nearest,
            self.usable_width(),
            self.glyphs(),
        ));
    }

    pub fn render_completions(&self, input: &str, items: &[Item]) {
        let mut frames = blocks::completion_palette(items, self.usable_width(), self.glyphs());
        frames.insert(0, prompt_frame(input));
        self.write_frames(&frames);
    }

    pub fn render_status(&self) {
        let s = &self.inner.services;
        let now = (s.now)();
        let activity = s.stats.activity();
        let bandwidth = s.stats.bandwidth(now, BANDWIDTH_SECONDS);
        let rows = status_rows(
            &activity,
            &bandwidth,
            (s.mesh_connected)(),
            now,
            self.glyphs(),
        );
        self.write_frames(&blocks::panel(
            "status",
            &rows,
            self.usable_width(),
            self.glyphs(),
        ));
    }

    pub fn render_update_help(&self, options: &[(&str, &str)]) {
        let names: Vec<_> = options
            .iter()
            .map(|(name, _)| format!("update {name}"))
            .collect();
        let commands: Vec<_> = names
            .iter()
            .zip(options)
            .map(|(name, (_, description))| PaletteCommand { name, description })
            .collect();
        self.write_frames(&blocks::command_palette(
            "update options",
            &commands,
            self.usable_width(),
            self.glyphs(),
        ));
    }

    fn write_block(&self, title: &str, rows: &[(String, String)]) {
        self.write_frames(&blocks::key_value(
            title,
            rows,
            self.usable_width(),
            self.glyphs(),
        ));
    }

    fn write_frames(&self, frames: &[Frame]) {
        let color = self.inner.services.ux.style().color;
        self.inner.services.ux.with_overlay_cleared(|out| {
            for frame in frames {
                let _ = write_frame_line(out, frame, color);
            }
        });
    }

    fn glyphs(&self) -> &'static Glyphs {
        glyphs(self.inner.services.ux.style().unicode)
    }

    fn usable_width(&self) -> usize {
        usable_width(self.inner.console_width())
    }
}

impl Inner {
    fn console_width(&self) -> usize {
        console_width((self.services.width)())
    }
}

pub fn console_width(window: Option<usize>) -> usize {
    window.map_or(DEFAULT_WIDTH, |w| w.saturating_sub(1).max(20))
}

pub fn usable_width(console: usize) -> usize {
    console.saturating_sub(2).max(20)
}

fn prompt_frame(input: &str) -> Frame {
    Frame::from_runs(vec![
        Run::new(PROMPT, vrcr_shared::console::Color::White),
        Run::new(input, vrcr_shared::console::Color::Gray),
    ])
}

pub fn status_rows(
    activity: &crate::stats::Activity,
    bandwidth: &Bandwidth,
    mesh_connected: bool,
    now: SystemTime,
    g: &Glyphs,
) -> Vec<(String, String, String)> {
    let recent = super::text::RECENT_WINDOW;
    let relay_active = activity.relay_active(now, recent);
    let upstream_active = activity.upstream_active(now, recent);

    let (video_state, video_detail) = if relay_active {
        (
            "serving VRChat",
            format!(
                "{} now, {} served",
                format_bytes_per_second(bandwidth.current),
                format_bytes(activity.relay_bytes)
            ),
        )
    } else {
        (
            "waiting",
            format!("{} served total", format_bytes(activity.relay_bytes)),
        )
    };
    let upstream_state = match (mesh_connected, upstream_active) {
        (false, _) => "reconnecting",
        (true, true) => "pulling media",
        (true, false) => "online idle",
    };
    let upstream_detail = if activity.upstream_relay_bytes > 0 {
        format!("{} received", format_bytes(activity.upstream_relay_bytes))
    } else {
        "no media pulled yet".to_owned()
    };
    let peak = if bandwidth.has_traffic() {
        format_bytes_per_second(bandwidth.peak)
    } else {
        "none yet".to_owned()
    };
    vec![
        (
            "local video".to_owned(),
            video_state.to_owned(),
            video_detail,
        ),
        (
            "resolver".to_owned(),
            upstream_state.to_owned(),
            upstream_detail,
        ),
        (
            "resolves".to_owned(),
            format!("{} total", activity.resolves_total),
            format!(
                "{} local video, {} cache hits",
                activity.resolves_via_lh_yt, activity.resolves_cache_hits
            ),
        ),
        (
            "last video".to_owned(),
            describe_age(activity.last_relay, now),
            "last bytes served to VRChat".to_owned(),
        ),
        (
            "last pull".to_owned(),
            describe_age(activity.last_upstream_relay, now),
            "last bytes received from the resolver".to_owned(),
        ),
        (
            "peak speed".to_owned(),
            peak,
            sparkline(&bandwidth.history, 12, g),
        ),
    ]
}

pub fn describe_age(at: Option<SystemTime>, now: SystemTime) -> String {
    let Some(at) = at else {
        return "not yet".to_owned();
    };
    let secs = now.duration_since(at).map_or(0, |d| d.as_secs());
    if secs < 2 {
        "now".to_owned()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", (secs / 3600).min(99))
    }
}

struct StatusOverlay {
    inner: Arc<Inner>,
    line: OverlayLine,
    gate: FrameGate,
}

impl Overlay for StatusOverlay {
    fn clear(&mut self, out: &mut dyn Write) -> io::Result<()> {
        self.line.clear(out)
    }

    fn render(&mut self, out: &mut dyn Write) -> io::Result<()> {
        let s = &self.inner.services;
        let now = (s.now)();
        let activity = s.stats.activity();
        let bandwidth = s.stats.bandwidth(now, BANDWIDTH_SECONDS);
        let width = self.inner.console_width();
        let style = s.ux.style();
        let (input, cursor, ghost) = {
            let st = self.inner.input.lock().unwrap_or_else(|e| e.into_inner());
            (st.buffer.text(), st.buffer.cursor(), st.ghost.clone())
        };
        let resized = self.gate.last_width() != Some(width);
        let frame = self.gate.frame(
            &activity,
            &bandwidth,
            now,
            &LineInput {
                mesh_connected: (s.mesh_connected)(),
                spinner: self.inner.spinner.load(Ordering::Relaxed),
                width,
                input: &input,
                animations: s.animations_available,
                unicode: style.unicode,
                ghost: &ghost,
                cursor: Some(cursor),
            },
        );
        let written = self.line.render_if_changed(out, &frame, style.color)?;
        if (written || resized)
            && let Some(column) = frame.cursor
            && column < width + 1
        {
            park_caret(out, column)?;
        }
        out.flush()
    }
}
