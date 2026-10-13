use vrcr_shared::win::{self, DefaultOutput};

const MMSYSERR_BADDEVICEID: u32 = 2;
const MMSYSERR_ALLOCATED: u32 = 4;
const MMSYSERR_NODRIVER: u32 = 6;

pub const NO_SOUND_FIX: &str = "pick a different default playback device in Windows Sound settings";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioState {
    Ok,
    InUse,
    NoDevice,
    Unknown,
}

impl AudioState {
    pub fn name(self) -> &'static str {
        match self {
            AudioState::Ok => "Ok",
            AudioState::InUse => "InUse",
            AudioState::NoDevice => "NoDevice",
            AudioState::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioCheck {
    pub state: AudioState,
    pub device: Option<String>,
}

impl AudioCheck {
    pub fn new(state: AudioState, device: Option<&str>) -> AudioCheck {
        AudioCheck {
            state,
            device: device.map(str::to_owned),
        }
    }

    pub fn blocks_playback(&self) -> bool {
        matches!(self.state, AudioState::InUse | AudioState::NoDevice)
    }
}

pub fn classify(open_result: u32) -> AudioState {
    match open_result {
        0 => AudioState::Ok,
        MMSYSERR_ALLOCATED => AudioState::InUse,
        MMSYSERR_BADDEVICEID | MMSYSERR_NODRIVER => AudioState::NoDevice,
        _ => AudioState::Unknown,
    }
}

pub fn from_probe(probe: DefaultOutput) -> AudioCheck {
    match probe {
        DefaultOutput::NoDevices | DefaultOutput::NoPreferred => AudioCheck::new(AudioState::NoDevice, None),
        DefaultOutput::Unknown => AudioCheck::new(AudioState::Unknown, None),
        DefaultOutput::Opened { result, device } => AudioCheck {
            state: classify(result),
            device,
        },
    }
}

pub fn check_default() -> AudioCheck {
    from_probe(win::probe_default_output())
}

pub fn no_sound_reason(audio: &AudioCheck) -> String {
    if audio.state == AudioState::NoDevice {
        return "Windows has no default playback device".to_owned();
    }
    let device = audio
        .device
        .as_deref()
        .filter(|d| !d.is_empty())
        .map(|d| format!(" '{d}'"))
        .unwrap_or_default();
    format!(
        "Windows' default playback device{device} is held by another app in exclusive mode (Voicemeeter, ASIO or KS outputs can do this)"
    )
}
