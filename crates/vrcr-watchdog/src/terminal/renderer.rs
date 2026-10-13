use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use vrcr_shared::console::{Component, Overlay, OverlayGuard, Ux};
use vrcr_shared::paths::AppPaths;

use super::blocks::{self, Item, PaletteCommand};
use super::commands::Command;
use super::input::InputBuffer;
use super::overlay::{OverlayLine, park_caret, write_frame_line};
use super::session::SessionStore;
use super::status::{FrameGate, LineInput, PROMPT};
use super::text::{Frame, Glyphs, Run, fast_refresh, glyphs, pad_left, sparkline};
use crate::display::{format_bytes, format_bytes_per_second};
use crate::maintenance::update::{self, UpdateStart};
use crate::settings::{self, AppSettings, Setting, Store};
use crate::stats::{Bandwidth, Stats};

pub const BANDWIDTH_SECONDS: usize = 24;
pub const DEFAULT_WIDTH: usize = 119;

pub type Flag = Arc<dyn Fn() -> bool + Send + Sync>;
pub type Action = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Services {
    pub ux: Ux,
    pub stats: Arc<Stats>,
    pub settings: Arc<Store>,
    pub mesh_connected: Flag,
    pub request_shutdown: Action,
    pub install_dir: PathBuf,
    pub paths: AppPaths,
    pub spawn_updater: fn(Process) -> io::Result<()>,
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
    session: Arc<SessionStore>,
    input: Mutex<InputState>,
    spinner: AtomicUsize,
}

#[derive(Clone)]
pub struct Renderer {
    inner: Arc<Inner>,
}

impl Renderer {
    pub fn new(services: Services, session: Arc<SessionStore>, input: InputBuffer) -> Renderer {
        Renderer {
            inner: Arc::new(Inner {
                services,
                session,
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

    pub fn request_shutdown(&self) {
        (self.inner.services.request_shutdown)();
    }

    pub fn start_update(&self) -> UpdateStart {
        let s = &self.inner.services;
        update::start_update(&s.install_dir, s.spawn_updater)
    }

    pub fn attach(&self) -> OverlayGuard {
        self.inner.services.ux.use_overlay(Box::new(StatusOverlay {
            inner: self.inner.clone(),
            line: OverlayLine::default(),
            gate: FrameGate::default(),
        }))
    }

    pub fn render_overlay(&self) {
        self.inner.services.ux.refresh_overlay();
    }

    pub fn fast_refresh(&self) -> bool {
        let s = &self.inner.services;
        fast_refresh(
            &s.stats.activity(),
            (s.now)(),
            &s.settings.snapshot(),
            s.animations_available,
        )
    }

    pub fn info(&self, message: &str) {
        self.inner.session.record_output("info", message);
        self.inner.services.ux.write(Component::Terminal, message);
    }

    pub fn success(&self, message: &str) {
        self.inner.session.record_output("success", message);
        self.inner.services.ux.success(Component::Terminal, message);
    }

    pub fn warn(&self, message: &str) {
        self.inner.session.record_output("warn", message);
        self.inner.services.ux.warn(Component::Terminal, message);
    }

    pub fn error(&self, message: &str) {
        self.inner.session.record_output("error", message);
        self.inner.services.ux.error(Component::Terminal, message);
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
                aliases: &c.aliases,
            })
            .collect();
        self.render_frames(
            "commands",
            &blocks::command_palette(&palette, self.usable_width(), self.glyphs()),
        );
        self.info("tab completes as you type; help <command> explains one.");
    }

    pub fn render_command_help(&self, command: &Command) {
        let mut rows = vec![
            ("command".to_owned(), command.name.clone()),
            ("about".to_owned(), command.description.clone()),
        ];
        if !command.aliases.is_empty() {
            rows.push(("also".to_owned(), command.aliases.join(", ")));
        }
        let arguments = if command.has_completer() {
            "tab completes them"
        } else {
            "none"
        };
        rows.push(("arguments".to_owned(), arguments.to_owned()));
        self.write_block("help", &rows);
    }

    pub fn render_unknown_command(&self, verb: &str, nearest: &[Item]) {
        self.warn(&format!("unknown command: {verb}"));
        if nearest.is_empty() {
            self.info("type help for the full list.");
            return;
        }
        self.render_frames(
            "did you mean",
            &blocks::completion_palette(nearest, self.usable_width(), self.glyphs()),
        );
    }

    pub fn render_completions(&self, input: &str, items: &[Item]) {
        let mut frames = blocks::completion_palette(items, self.usable_width(), self.glyphs());
        let first = frames.first().map(|f| f.plain.clone()).unwrap_or_default();
        self.inner.session.record_output("matches", &first);
        frames.insert(0, prompt_frame(input));
        self.write_frames(&frames);
    }

    pub fn render_history(&self, commands: &[String]) {
        if commands.is_empty() {
            self.info("history: no commands in this session yet.");
            return;
        }
        let rows: Vec<(String, String)> = commands
            .iter()
            .enumerate()
            .map(|(i, c)| (pad_left(&(i + 1).to_string(), 2), c.clone()))
            .collect();
        self.write_block("history", &rows);
    }

    pub fn render_tools(&self) {
        let mesh = if (self.inner.services.mesh_connected)() {
            "online"
        } else {
            "reconnecting"
        };
        let rows = [
            (
                "VRChat hook",
                "ready",
                "keeps game video requests flowing through VRCResolver",
            ),
            (
                "resolver mesh",
                mesh,
                "resolves playback URLs and fallback decisions",
            ),
            (
                "local video",
                "ready",
                "serves localhost.youtube.com playback to the game",
            ),
            (
                "hosts entry",
                "managed",
                "keeps localhost.youtube.com pointing at this PC",
            ),
            (
                "diagnostics",
                "recording",
                "writes logs, crash snapshots, and playback feedback",
            ),
        ];
        self.write_table("managed subsystems", &owned3(&rows));
    }

    pub fn render_diagnostics(&self, session: &SessionStore) {
        let s = &self.inner.services;
        let shown = |p: &std::path::Path| p.display().to_string();
        let rows = vec![
            ("install".to_owned(), shown(&s.install_dir)),
            ("state".to_owned(), shown(&s.paths.state_root())),
            ("settings".to_owned(), shown(s.settings.path())),
            ("logs".to_owned(), shown(&s.paths.logs_dir())),
            ("crashes".to_owned(), shown(&s.paths.crashes_dir())),
            ("terminal history".to_owned(), shown(session.history_path())),
            ("this session".to_owned(), shown(session.session_path())),
        ];
        self.render_frames(
            "diagnostics",
            &blocks::key_value("diagnostics", &rows, self.usable_width(), self.glyphs()),
        );
    }

    pub fn render_status(&self) {
        let s = &self.inner.services;
        let settings = s.settings.snapshot();
        let now = (s.now)();
        let activity = s.stats.activity();
        let bandwidth = s.stats.bandwidth(now, BANDWIDTH_SECONDS);
        let rows = status_rows(
            &activity,
            &bandwidth,
            (s.mesh_connected)(),
            &settings,
            now,
            self.glyphs(),
        );
        self.render_frames(
            "panel",
            &blocks::panel("status", &rows, self.usable_width(), self.glyphs()),
        );
    }

    pub fn render_settings(&self) {
        let snapshot = self.settings().snapshot();
        let rows: Vec<(String, String, String)> = settings::ALL
            .iter()
            .map(|s| {
                let suffix = if s.restart_required { " (restart)" } else { "" };
                (
                    s.key.to_owned(),
                    format!("{}{suffix}", s.get(&snapshot)),
                    s.description.to_owned(),
                )
            })
            .collect();
        self.write_table("settings", &rows);
    }

    pub fn render_setting(&self, setting: &Setting) {
        let snapshot = self.settings().snapshot();
        let suffix = if setting.restart_required {
            " (takes effect next launch)"
        } else {
            ""
        };
        let rows = vec![
            ("name".to_owned(), setting.key.to_owned()),
            (
                "current".to_owned(),
                format!("{}{suffix}", setting.get(&snapshot)),
            ),
            ("enter".to_owned(), setting.choices().join(", ")),
            ("about".to_owned(), setting.description.to_owned()),
        ];
        self.write_block("setting", &rows);
    }

    pub fn render_settings_help(&self) {
        let rows = [
            ("settings", "list settings"),
            ("settings get <name>", "show one setting"),
            ("settings set <name> <value>", "change a setting"),
            ("settings <name> <value>", "change a setting"),
            ("settings reset <name>", "reset one setting"),
            ("settings reset all", "reset every setting"),
            ("example", "settings status-line off"),
            ("example", "settings high-quality on"),
        ];
        let rows: Vec<(String, String)> = rows
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect();
        self.write_block("settings help", &rows);
    }

    fn write_block(&self, title: &str, rows: &[(String, String)]) {
        self.render_frames(
            "block",
            &blocks::key_value(title, rows, self.usable_width(), self.glyphs()),
        );
    }

    fn write_table(&self, title: &str, rows: &[(String, String, String)]) {
        self.render_frames(
            "table",
            &blocks::table(title, rows, self.usable_width(), self.glyphs()),
        );
    }

    fn render_frames(&self, kind: &str, frames: &[Frame]) {
        let first = frames.first().map(|f| f.plain.as_str()).unwrap_or("");
        self.inner.session.record_output(kind, first);
        self.write_frames(frames);
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

fn owned3(rows: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    rows.iter()
        .map(|(a, b, c)| ((*a).to_owned(), (*b).to_owned(), (*c).to_owned()))
        .collect()
}

pub fn status_rows(
    activity: &crate::stats::Activity,
    bandwidth: &Bandwidth,
    mesh_connected: bool,
    settings: &AppSettings,
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
    let (quality, quality_detail) = if settings.playback.high_quality {
        (
            "highest available",
            "up to 4K where the source has it -- settings high-quality off to revert",
        )
    } else {
        (
            "as VRChat asks",
            "settings high-quality on to take the best rung a source offers",
        )
    };
    vec![
        ("local video".to_owned(), video_state.to_owned(), video_detail),
        ("resolver".to_owned(), upstream_state.to_owned(), upstream_detail),
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
        (
            "terminal".to_owned(),
            if settings.terminal.status_line {
                "live prompt"
            } else {
                "prompt only"
            }
            .to_owned(),
            describe_terminal(settings).to_owned(),
        ),
        (
            "quality".to_owned(),
            quality.to_owned(),
            quality_detail.to_owned(),
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

pub fn describe_terminal(settings: &AppSettings) -> &'static str {
    if !settings.terminal.status_line {
        "status line off"
    } else if settings.terminal.animations {
        "status line on, motion on"
    } else {
        "status line on, motion off"
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
        let settings = s.settings.snapshot();
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
                status_line: settings.terminal.status_line,
                animations: settings.terminal.animations && s.animations_available,
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
