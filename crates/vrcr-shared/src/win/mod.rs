#[cfg(windows)]
mod registry;

#[cfg(windows)]
pub use registry::{HkcuKey, local_hms, process_alive};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineStore {
    Root,
    My,
}

impl MachineStore {
    pub fn name(self) -> &'static str {
        match self {
            MachineStore::Root => "ROOT",
            MachineStore::My => "MY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elevated {
    Exited(u32),
    Declined,
    TimedOut,
    Failed(String),
}

#[cfg(windows)]
mod certificates;

#[cfg(windows)]
pub use certificates::{
    dpapi_protect, dpapi_unprotect, machine_store_add, machine_store_certs, machine_store_remove,
    run_elevated,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultOutput {
    NoDevices,
    NoPreferred,
    Unknown,
    Opened { result: u32, device: Option<String> },
}

#[cfg(windows)]
mod audio;

#[cfg(windows)]
pub use audio::probe_default_output;

#[cfg(not(windows))]
pub fn probe_default_output() -> DefaultOutput {
    DefaultOutput::Unknown
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub exe: String,
}

#[cfg(windows)]
mod processes;

#[cfg(windows)]
pub use processes::{list_processes, process_start_time};

#[cfg(windows)]
mod process_control;

#[cfg(windows)]
pub use process_control::{
    free_disk_bytes, ignore_ctrl_c, process_image_path, send_console_ctrl_c, terminate_process,
    wait_for_exit,
};

#[cfg(not(windows))]
pub fn ignore_ctrl_c(_ignore: bool) -> bool {
    false
}

#[cfg(not(windows))]
pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(not(windows))]
pub fn wait_for_exit(pid: u32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while process_alive(pid) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    true
}

#[cfg(not(windows))]
pub fn terminate_process(_pid: u32) -> bool {
    false
}

#[cfg(not(windows))]
pub fn send_console_ctrl_c(_pid: u32) -> bool {
    false
}

#[cfg(not(windows))]
pub fn free_disk_bytes(_path: &std::path::Path) -> Option<u64> {
    None
}

#[cfg(not(windows))]
pub fn list_processes() -> Vec<ProcessInfo> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn process_start_time(_pid: u32) -> Option<std::time::SystemTime> {
    None
}

pub fn process_stem(exe: &str) -> &str {
    let len = exe.len();
    if len > 4 && exe.is_char_boundary(len - 4) && exe[len - 4..].eq_ignore_ascii_case(".exe") {
        &exe[..len - 4]
    } else {
        exe
    }
}

pub fn describe_parent(processes: &[ProcessInfo], own_pid: u32) -> Option<String> {
    let me = processes.iter().find(|p| p.pid == own_pid)?;
    let parent = processes
        .iter()
        .find(|p| p.pid == me.parent_pid && p.pid != own_pid)?;
    Some(format!("{} pid={}", process_stem(&parent.exe), parent.pid))
}

#[cfg(windows)]
mod media_foundation;

#[cfg(windows)]
pub use media_foundation::MediaFoundation;

#[cfg(not(windows))]
pub struct MediaFoundation(());

#[cfg(not(windows))]
impl MediaFoundation {
    pub fn start() -> Option<MediaFoundation> {
        None
    }

    pub fn has_video_decoder(&self, _fourcc: u32) -> Option<bool> {
        None
    }
}

#[cfg(not(windows))]
pub fn local_hms() -> (u16, u16, u16) {
    let s = crate::logutil::UtcStamp::at(std::time::SystemTime::now());
    let part = |v: u64| u16::try_from(v).unwrap_or(0);
    (part(s.hour), part(s.minute), part(s.second))
}

#[cfg(windows)]
pub struct RegDword<'a> {
    pub key: &'a HkcuKey,
    pub name: &'a str,
}

#[cfg(windows)]
impl crate::wininet::DwordStore for RegDword<'_> {
    fn get(&self) -> Option<u32> {
        self.key.get_dword(self.name)
    }

    fn set(&mut self, value: u32) -> std::io::Result<()> {
        self.key.set_dword(self.name, value)
    }

    fn delete(&mut self) -> std::io::Result<()> {
        self.key.delete_value(self.name)
    }
}

#[cfg(not(windows))]
pub fn process_alive(pid: u32) -> bool {
    pid == 0 || std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u16,
    pub ch: u16,
    pub ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlEvent {
    CtrlC,
    Close,
    Logoff,
    Shutdown,
}

pub fn ctrl_event(code: u32) -> Option<CtrlEvent> {
    match code {
        0 | 1 => Some(CtrlEvent::CtrlC),
        2 => Some(CtrlEvent::Close),
        5 => Some(CtrlEvent::Logoff),
        6 => Some(CtrlEvent::Shutdown),
        _ => None,
    }
}

pub const QUICK_EDIT_MODE: u32 = 0x0040;
pub const EXTENDED_FLAGS: u32 = 0x0080;

pub fn without_quick_edit(mode: u32) -> u32 {
    (mode & !QUICK_EDIT_MODE) | EXTENDED_FLAGS
}

pub type CtrlHandler = Box<dyn Fn(CtrlEvent) + Send + Sync>;

static CTRL_HANDLER: std::sync::OnceLock<CtrlHandler> = std::sync::OnceLock::new();

#[cfg(windows)]
mod console;

#[cfg(windows)]
pub use console::{
    KeyReader, MutexError, NamedMutex, console_width, disable_quick_edit, enable_vt_output,
    install_ctrl_handler,
};

#[cfg(not(windows))]
pub fn enable_vt_output() -> bool {
    true
}

#[cfg(not(windows))]
pub fn disable_quick_edit() -> &'static str {
    "unavailable"
}

#[cfg(not(windows))]
pub fn console_width() -> Option<usize> {
    None
}

#[cfg(not(windows))]
pub fn install_ctrl_handler(handler: CtrlHandler) -> bool {
    let _ = CTRL_HANDLER.set(handler);
    false
}

#[cfg(windows)]
mod version;

#[cfg(windows)]
pub use version::os_description;

#[cfg(not(windows))]
pub fn os_description() -> String {
    std::env::consts::OS.to_owned()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessUsage {
    pub cpu_100ns: u64,
    pub working_set: u64,
    pub peak_working_set: u64,
    pub private_bytes: u64,
    pub handles: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryStatus {
    pub total: u64,
    pub available: u64,
}

#[cfg(windows)]
mod usage;

#[cfg(windows)]
pub use usage::{cpu_name, memory_status, process_usage};

#[cfg(not(windows))]
pub fn process_usage() -> Option<ProcessUsage> {
    None
}

#[cfg(not(windows))]
pub fn memory_status() -> Option<MemoryStatus> {
    None
}

#[cfg(not(windows))]
pub fn cpu_name() -> Option<String> {
    None
}
