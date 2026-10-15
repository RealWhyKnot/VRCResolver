use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Mesh,
    Ipc,
    Hosts,
    Patch,
    Wrapper,
    Heartbeat,
    Relay,
    Terminal,
    Codec,
    Report,
    Update,
    VrcLog,
    YtDlp,
    Shutdown,
}

impl Component {
    pub fn tag(self) -> &'static str {
        match self {
            Component::Mesh => "[mesh]",
            Component::Ipc => "[ipc]",
            Component::Hosts => "[hosts]",
            Component::Patch => "[patch]",
            Component::Wrapper => "[wrapper]",
            Component::Heartbeat => "[heartbeat]",
            Component::Relay => "[relay]",
            Component::Terminal => "[terminal]",
            Component::Codec => "[codec]",
            Component::Report => "[report]",
            Component::Update => "[update]",
            Component::VrcLog => "[vrclog]",
            Component::YtDlp => "[yt-dlp]",
            Component::Shutdown => "[shutdown]",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Component::Mesh => Color::DarkCyan,
            Component::Ipc
            | Component::Hosts
            | Component::Wrapper
            | Component::Heartbeat
            | Component::Report
            | Component::VrcLog => Color::DarkGray,
            Component::Shutdown => Color::DarkRed,
            Component::Patch
            | Component::Relay
            | Component::Terminal
            | Component::Codec
            | Component::Update
            | Component::YtDlp => Color::Gray,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveStatus {
    Resolved,
    Fallback,
    Failed,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    DarkCyan,
    DarkGray,
    DarkRed,
    Gray,
    Green,
    Yellow,
    Red,
    White,
}

impl Color {
    pub fn sgr(self) -> &'static str {
        match self {
            Color::DarkCyan => "\x1b[36m",
            Color::DarkGray => "\x1b[90m",
            Color::DarkRed => "\x1b[31m",
            Color::Gray => "\x1b[37m",
            Color::Green => "\x1b[92m",
            Color::Yellow => "\x1b[93m",
            Color::Red => "\x1b[91m",
            Color::White => "\x1b[97m",
        }
    }
}

const RESET: &str = "\x1b[0m";
const HOST_WIDTH: usize = 28;
const PLAYER_WIDTH: usize = 14;
const OUTCOME_WIDTH: usize = 9;
const DIVIDER: &str = "============================================================";
const SUBDIVIDER: &str = "------------------------------------------------------------";

pub trait Overlay: Send {
    fn clear(&mut self, out: &mut dyn Write) -> io::Result<()>;
    fn render(&mut self, out: &mut dyn Write) -> io::Result<()>;
}

pub type Clock = fn() -> String;

pub fn local_clock() -> String {
    let (h, m, s) = crate::win::local_hms();
    format!("{h:02}:{m:02}:{s:02}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub color: bool,
    pub unicode: bool,
}

impl Style {
    pub fn plain() -> Style {
        Style {
            color: false,
            unicode: false,
        }
    }

    pub fn detect(redirected: bool, no_color: Option<&str>, ascii_terminal: Option<&str>) -> Style {
        if redirected {
            return Style::plain();
        }
        let ascii = ascii_terminal.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        Style {
            color: no_color.is_none_or(str::is_empty),
            unicode: !ascii,
        }
    }

    pub fn bullet(self) -> &'static str {
        if self.unicode { "\u{2022}" } else { "*" }
    }
}

#[derive(Clone)]
pub struct Ux {
    inner: Arc<Mutex<State>>,
    style: Style,
    clock: Clock,
}

struct State {
    out: Box<dyn Write + Send>,
    overlay: Option<(u64, Box<dyn Overlay>)>,
    tee: Option<crate::log::Log>,
    next_id: u64,
}

pub struct OverlayGuard {
    ux: Ux,
    id: u64,
}

impl Drop for OverlayGuard {
    fn drop(&mut self) {
        let mut st = self.ux.state();
        if st.overlay.as_ref().is_some_and(|(id, _)| *id == self.id) {
            clear_overlay(&mut st);
            st.overlay = None;
        }
    }
}

impl Ux {
    pub fn new(out: Box<dyn Write + Send>, style: Style, clock: Clock) -> Ux {
        Ux {
            inner: Arc::new(Mutex::new(State {
                out,
                overlay: None,
                tee: None,
                next_id: 1,
            })),
            style,
            clock,
        }
    }

    pub fn stdout() -> Ux {
        use std::io::IsTerminal;
        let redirected = !io::stdout().is_terminal();
        let no_color = std::env::var("NO_COLOR").ok();
        let ascii = crate::legacy::env_with_fallback("ASCII_TERMINAL");
        Ux::new(
            Box::new(io::stdout()),
            Style::detect(redirected, no_color.as_deref(), ascii.as_deref()),
            local_clock,
        )
    }

    pub fn sink() -> Ux {
        Ux::new(Box::new(io::sink()), Style::plain(), local_clock)
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn use_overlay(&self, overlay: Box<dyn Overlay>) -> OverlayGuard {
        let mut st = self.state();
        clear_overlay(&mut st);
        let id = st.next_id;
        st.next_id += 1;
        st.overlay = Some((id, overlay));
        render_overlay(&mut st);
        OverlayGuard { ux: self.clone(), id }
    }

    pub fn with_lock<R>(&self, f: impl FnOnce(&mut dyn Write) -> R) -> R {
        let mut st = self.state();
        f(&mut *st.out)
    }

    pub fn tee_to(&self, log: crate::log::Log) {
        self.state().tee = Some(log);
    }

    pub fn with_overlay_cleared<R>(&self, f: impl FnOnce(&mut dyn Write) -> R) -> R {
        let mut st = self.state();
        clear_overlay(&mut st);
        let r = f(&mut *st.out);
        let _ = st.out.flush();
        render_overlay(&mut st);
        r
    }

    pub fn refresh_overlay(&self) {
        let mut st = self.state();
        render_overlay(&mut st);
    }

    pub fn write(&self, c: Component, body: &str) {
        self.stamped(c.color(), &format!("{} {} {body}", self.style.bullet(), c.tag()));
    }

    pub fn success(&self, c: Component, body: &str) {
        self.stamped(
            Color::Green,
            &format!("{} {} {body}", self.style.bullet(), c.tag()),
        );
    }

    pub fn warn(&self, c: Component, body: &str) {
        self.stamped(
            Color::Yellow,
            &format!("{} {}[warn] {body}", self.style.bullet(), c.tag()),
        );
    }

    pub fn error(&self, c: Component, body: &str) {
        self.stamped(
            Color::Red,
            &format!("{} {}[err] {body}", self.style.bullet(), c.tag()),
        );
    }

    pub fn fatal(&self, body: &str) {
        self.stamped(Color::Red, &format!("{} [FATAL] {body}", self.style.bullet()));
    }

    pub fn resolve_outcome(&self, o: &Outcome<'_>) {
        let (color, line) = resolve_outcome_line(self.style.bullet(), o);
        self.stamped(color, &line);
    }

    pub fn wrapper_fallback(&self, host: &str, reason: &str, elapsed_ms: u64) {
        let line = format!(
            "{} [wrapper] !!  {:<OUTCOME_WIDTH$} {}       {reason}   elapsed={elapsed_ms}ms",
            self.style.bullet(),
            "fallback",
            truncate_pad(host, HOST_WIDTH)
        );
        self.stamped(Color::Yellow, &line);
    }

    pub fn banner(&self, b: &Banner<'_>) {
        let mut st = self.state();
        for (color, line) in banner_lines(b) {
            raw(&mut st, self.style, color, &line);
        }
    }

    pub fn phase_divider(&self, label: &str) {
        self.stamped(Color::DarkGray, &phase_divider_line(label));
    }

    fn stamped(&self, color: Color, body: &str) {
        let line = format!("[{}] {body}", (self.clock)());
        let mut st = self.state();
        raw(&mut st, self.style, color, &line);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn raw(st: &mut State, style: Style, color: Color, text: &str) {
    if let Some(log) = &st.tee {
        log.line(text);
    }
    clear_overlay(st);
    let _ = if style.color {
        writeln!(st.out, "{}{text}{RESET}", color.sgr())
    } else {
        writeln!(st.out, "{text}")
    };
    let _ = st.out.flush();
    render_overlay(st);
}

fn clear_overlay(st: &mut State) {
    let State { out, overlay, .. } = st;
    if let Some((_, o)) = overlay
        && o.clear(&mut **out).is_err()
    {
        *overlay = None;
    }
}

fn render_overlay(st: &mut State) {
    let State { out, overlay, .. } = st;
    if let Some((_, o)) = overlay
        && o.render(&mut **out).is_err()
    {
        *overlay = None;
    }
}

pub struct Outcome<'a> {
    pub host: &'a str,
    pub player: &'a str,
    pub status: ResolveStatus,
    pub via_cache: bool,
    pub via_lh_yt: bool,
    pub elapsed: Duration,
    pub reason: Option<&'a str>,
}

pub fn resolve_outcome_line(bullet: &str, o: &Outcome<'_>) -> (Color, String) {
    let (color, token, word) = match o.status {
        ResolveStatus::Resolved => (
            Color::Green,
            "OK",
            if o.via_cache { "cached" } else { "resolved" },
        ),
        ResolveStatus::Fallback => (Color::Yellow, "!!", "fallback"),
        ResolveStatus::Failed => (Color::Red, "XX", "failed"),
        ResolveStatus::Unexpected => (Color::DarkGray, "??", "unknown"),
    };
    let yt = if o.via_lh_yt { " [yt]" } else { "     " };
    let reason = o
        .reason
        .filter(|r| !r.is_empty())
        .map(|r| format!("   {r}"))
        .unwrap_or_default();
    let line = format!(
        "{bullet} [resolve] {token}  {word:<OUTCOME_WIDTH$} {}{yt}  {}  {:>6}{reason}",
        truncate_pad(o.host, HOST_WIDTH),
        truncate_pad(o.player, PLAYER_WIDTH),
        format_elapsed(o.elapsed)
    );
    (color, line)
}

pub struct Banner<'a> {
    pub version: &'a str,
    pub channel: Option<&'a str>,
    pub sha: &'a str,
    pub build_time: &'a str,
    pub dev: bool,
    pub paths: &'a [(&'a str, String)],
}

pub fn banner_lines(b: &Banner<'_>) -> Vec<(Color, String)> {
    let channel = b.channel.map(|c| format!(" ({c})")).unwrap_or_default();
    let mut lines = vec![
        (Color::DarkGray, DIVIDER.to_owned()),
        (Color::White, format!("  VRCResolver {}{channel}", b.version)),
        (Color::Gray, "  local video relay for VRChat".to_owned()),
        (Color::Gray, format!("  sha {}  built {}", b.sha, b.build_time)),
    ];
    if b.dev {
        lines.push((
            Color::Yellow,
            "  debug: on (relay trace and hitch warnings here, timings and perf numbers in the log)"
                .to_owned(),
        ));
    }
    lines.push((Color::DarkGray, SUBDIVIDER.to_owned()));
    let width = b.paths.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0) + 2;
    for (label, value) in b.paths {
        lines.push((Color::Gray, format!("  {:<width$} {value}", format!("{label}:"))));
    }
    lines.push((Color::DarkGray, DIVIDER.to_owned()));
    lines.push((Color::Gray, String::new()));
    lines
}

pub fn phase_divider_line(label: &str) -> String {
    let body = format!(" {label} ");
    let pad = 60usize.saturating_sub(body.chars().count());
    let left = pad / 2;
    format!("{}{body}{}", "-".repeat(left), "-".repeat(pad - left))
}

pub fn truncate_pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len > width {
        if width >= 4 {
            let head: String = s.chars().take(width - 2).collect();
            format!("{head}..")
        } else {
            s.chars().take(width).collect()
        }
    } else {
        format!("{s:<width$}")
    }
}

pub fn format_elapsed(t: Duration) -> String {
    let seconds = t.as_secs_f64();
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    let total = t.as_secs();
    format!("{}m{}s", total / 60, total % 60)
}
