use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use vrcr_shared::console::OverlayGuard;
use vrcr_shared::win::KeyEvent;

use super::commands::{Context, Registry};
use super::input::InputBuffer;
use super::renderer::{Renderer, Services};
use super::session::SessionStore;
use super::text::{ACTIVE_REFRESH, CommandLine, IDLE_REFRESH};

pub const STOP_WAIT: Duration = Duration::from_millis(500);
pub const INPUT_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Backspace,
    Delete,
    Escape,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    CtrlU,
    CtrlL,
    CtrlD,
    CtrlC,
    Char(char),
    Ignored,
}

#[derive(Debug, Default)]
pub struct KeyDecoder {
    high: Option<u16>,
}

impl KeyDecoder {
    pub fn decode(&mut self, ev: KeyEvent) -> Key {
        if ev.ctrl {
            match ev.vk {
                0x55 => return Key::CtrlU,
                0x4C => return Key::CtrlL,
                0x44 => return Key::CtrlD,
                0x43 => return Key::CtrlC,
                _ => {}
            }
        }
        match ev.vk {
            0x0D => return Key::Enter,
            0x08 => return Key::Backspace,
            0x2E => return Key::Delete,
            0x1B => return Key::Escape,
            0x09 => return Key::Tab,
            0x25 => return Key::Left,
            0x27 => return Key::Right,
            0x24 => return Key::Home,
            0x23 => return Key::End,
            0x26 => return Key::Up,
            0x28 => return Key::Down,
            _ => {}
        }
        match ev.ch {
            0xD800..=0xDBFF => {
                self.high = Some(ev.ch);
                Key::Ignored
            }
            0xDC00..=0xDFFF => match self.high.take() {
                Some(high) => char::decode_utf16([high, ev.ch])
                    .next()
                    .and_then(Result::ok)
                    .map_or(Key::Ignored, Key::Char),
                None => Key::Ignored,
            },
            unit => {
                self.high = None;
                match char::from_u32(u32::from(unit)) {
                    Some(c) if !c.is_control() => Key::Char(c),
                    _ => Key::Ignored,
                }
            }
        }
    }
}

pub fn interactive_allowed(stdin_tty: bool, stdout_tty: bool, disabled: Option<&str>) -> bool {
    stdin_tty && stdout_tty && !disabled.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub fn animations_allowed(stdin_tty: bool, color: bool, disabled: Option<&str>) -> bool {
    stdin_tty && color && !disabled.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub struct Core {
    renderer: Renderer,
    session: Arc<SessionStore>,
    commands: Registry,
    stopped: AtomicBool,
}

impl Core {
    pub fn new(services: Services, session: Arc<SessionStore>) -> Core {
        let input = InputBuffer::new(session.load_history());
        Core {
            renderer: Renderer::new(services, session.clone(), input),
            session,
            commands: Registry::with_defaults(),
            stopped: AtomicBool::new(false),
        }
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    pub fn commands(&self) -> &Registry {
        &self.commands
    }

    pub fn input_text(&self) -> String {
        self.renderer.input().buffer.text()
    }

    pub fn ghost(&self) -> String {
        self.renderer.input().ghost.clone()
    }

    pub fn handle_key(&self, key: Key) {
        match key {
            Key::CtrlU | Key::Escape => self.edit(|b| b.clear(), false),
            Key::CtrlL => self.renderer.clear_screen(),
            Key::CtrlD => {
                if self.input_text().is_empty() {
                    self.renderer.request_shutdown();
                }
            }
            Key::CtrlC => {
                self.edit_quiet(|b| b.clear(), false);
                self.renderer.warn("input cancelled.");
            }
            Key::Enter => self.submit(),
            Key::Backspace => self.edit(InputBuffer::backspace, true),
            Key::Delete => self.edit(InputBuffer::delete, true),
            Key::Tab => self.complete(true),
            Key::Left => self.edit(InputBuffer::move_left, false),
            Key::Right => {
                let accept = {
                    let st = self.renderer.input();
                    st.buffer.at_end() && !st.ghost.is_empty()
                };
                if accept {
                    self.complete(false);
                } else {
                    self.edit(InputBuffer::move_right, false);
                }
            }
            Key::Home => self.edit(InputBuffer::move_home, false),
            Key::End => self.edit(InputBuffer::move_end, true),
            Key::Up => self.edit(InputBuffer::previous_history, false),
            Key::Down => self.edit(InputBuffer::next_history, false),
            Key::Char(c) => self.edit(|b| b.append(c), true),
            Key::Ignored => {}
        }
    }

    fn edit_quiet(&self, f: impl FnOnce(&mut InputBuffer), suggest: bool) {
        let mut st = self.renderer.input();
        f(&mut st.buffer);
        st.ghost = if suggest && st.buffer.at_end() {
            self.commands.suggest(&st.buffer.text())
        } else {
            String::new()
        };
    }

    fn edit(&self, f: impl FnOnce(&mut InputBuffer), suggest: bool) {
        self.edit_quiet(f, suggest);
        self.redraw();
    }

    fn redraw(&self) {
        if !self.stopped.load(Ordering::SeqCst) {
            self.renderer.render_overlay();
        }
    }

    fn complete(&self, show_when_ambiguous: bool) {
        let text = self.input_text();
        let completion = self.commands.complete(&text);
        if !completion.replacement.is_empty() {
            self.edit(|b| b.set(&completion.replacement), true);
            return;
        }
        if show_when_ambiguous && !completion.suggestions.is_empty() {
            self.renderer.render_completions(&text, &completion.suggestions);
        } else {
            self.redraw();
        }
    }

    fn submit(&self) {
        let command_text = {
            let mut st = self.renderer.input();
            st.ghost.clear();
            let text = st.buffer.take().trim().to_owned();
            if !text.is_empty() {
                st.buffer.remember(&text);
            }
            text
        };
        if command_text.is_empty() {
            self.redraw();
            return;
        }
        self.session.record_command(&command_text);
        self.renderer.echo_command(&command_text);
        let parsed = CommandLine::parse(&command_text);
        let Some(command) = self.commands.get(&parsed.verb) else {
            self.renderer
                .render_unknown_command(&parsed.verb, &self.commands.nearest(&parsed.verb, 3));
            return;
        };
        let ctx = Context {
            renderer: &self.renderer,
            session: &self.session,
            commands: &self.commands,
        };
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| command.run(&ctx, &parsed.args))) {
            let reason = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown".to_owned());
            self.renderer.error(&format!("command failed: {reason}"));
        }
    }
}

struct StopSignal {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl StopSignal {
    fn is_set(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let guard = self.stopped.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = self
            .wake
            .wait_timeout_while(guard, timeout, |s| !*s)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

pub struct Terminal {
    core: Arc<Core>,
    signal: Arc<StopSignal>,
    overlay: Mutex<Option<OverlayGuard>>,
    done: Mutex<Vec<Receiver<()>>>,
    started: AtomicBool,
}

impl Terminal {
    pub fn new(services: Services, session: Arc<SessionStore>) -> Terminal {
        Terminal {
            core: Arc::new(Core::new(services, session)),
            signal: Arc::new(StopSignal {
                stopped: Mutex::new(false),
                wake: Condvar::new(),
            }),
            overlay: Mutex::new(None),
            done: Mutex::new(Vec::new()),
            started: AtomicBool::new(false),
        }
    }

    pub fn core(&self) -> &Core {
        &self.core
    }

    pub fn start(&self) -> bool {
        if self.started.swap(true, Ordering::SeqCst) || !key_input_available() {
            return false;
        }
        self.core.session.start();
        *self.overlay.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.core.renderer.attach());
        self.core
            .renderer
            .success("interactive terminal ready; type /help for commands.");
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        done.push(self.spawn("terminal-input", input_loop));
        done.push(self.spawn("terminal-render", render_loop));
        true
    }

    fn spawn(&self, name: &str, body: fn(&Core, &StopSignal)) -> Receiver<()> {
        let (tx, rx) = channel();
        let core = self.core.clone();
        let signal = self.signal.clone();
        let spawned = thread::Builder::new().name(name.to_owned()).spawn(move || {
            body(&core, &signal);
            let _ = tx.send(());
        });
        if spawned.is_err() {
            let (tx, rx) = channel();
            let _ = tx.send(());
            return rx;
        }
        rx
    }

    pub fn stop(&self) {
        self.core.stopped.store(true, Ordering::SeqCst);
        self.signal.set();
        drop(self.overlay.lock().unwrap_or_else(|e| e.into_inner()).take());
        self.core.session.stop();
        let waiting: Vec<Receiver<()>> =
            std::mem::take(&mut *self.done.lock().unwrap_or_else(|e| e.into_inner()));
        for rx in waiting {
            let _ = rx.recv_timeout(STOP_WAIT);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.core.stopped.store(true, Ordering::SeqCst);
        self.signal.set();
        drop(self.overlay.lock().unwrap_or_else(|e| e.into_inner()).take());
    }
}

fn input_loop(core: &Core, signal: &StopSignal) {
    #[cfg(windows)]
    {
        let Some(reader) = vrcr_shared::win::KeyReader::open() else {
            return;
        };
        let mut decoder = KeyDecoder::default();
        while !signal.is_set() {
            match reader.read(INPUT_POLL) {
                Ok(Some(ev)) => {
                    if signal.is_set() {
                        return;
                    }
                    core.handle_key(decoder.decode(ev));
                }
                Ok(None) => {}
                Err(_) => return,
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (core, signal);
    }
}

fn render_loop(core: &Core, signal: &StopSignal) {
    loop {
        let interval = if core.renderer.fast_refresh() {
            ACTIVE_REFRESH
        } else {
            IDLE_REFRESH
        };
        if signal.wait(interval) {
            return;
        }
        core.renderer.tick();
        core.renderer.render_overlay();
    }
}

fn key_input_available() -> bool {
    #[cfg(windows)]
    {
        vrcr_shared::win::KeyReader::open().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
