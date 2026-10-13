use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use vrcr_shared::console::{Component, Ux};
use vrcr_shared::log::Log;
use vrcr_shared::logutil::sanitize_for_console;
use vrcr_shared::win::MediaFoundation;
use vrcr_shared::wire::EXTENSION_BACKED_VIDEO_CODECS;

pub const PER_CODEC_TIMEOUT: Duration = Duration::from_secs(60);
pub const APPX_TIMEOUT: Duration = Duration::from_secs(10);
pub const TRANSIENT_RETRY_WINDOW: Duration = Duration::from_secs(24 * 3600);
pub const PERMANENT_RETRY_WINDOW: Duration = Duration::from_secs(7 * 24 * 3600);
pub const STATE_FILE: &str = "codec-state.json";
const WINGET_OUTPUT_CAP: usize = 8 * 1024;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const FAIL_WINGET_MISSING: &str = "winget_missing";
pub const FAIL_NOT_ENTITLED: &str = "not_entitled";
pub const FAIL_NOT_FOUND: &str = "not_found";
pub const FAIL_SOURCE_UNAVAILABLE: &str = "msstore_source_unavailable";
pub const FAIL_TIMEOUT: &str = "timeout";
pub const FAIL_UNVERIFIED_INSTALL: &str = "install_unverified";
pub const FAIL_UNKNOWN: &str = "unknown";

pub const STATUS_INSTALLED: &str = "installed";
pub const STATUS_FAILED: &str = "failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codec {
    pub name: &'static str,
    pub package: &'static str,
    pub probe: &'static str,
    pub store_ids: &'static [&'static str],
}

pub const REQUIRED: [Codec; 3] = [
    Codec {
        name: "AV1 Video Extension",
        package: "Microsoft.AV1VideoExtension",
        probe: "av1",
        store_ids: &["9MVZQVXJBQ9V"],
    },
    Codec {
        name: "HEVC Video Extensions",
        package: "Microsoft.HEVCVideoExtension",
        probe: "h265",
        store_ids: &["9NMZLZ57R3T7", "9N4WGH0Z6VHQ"],
    },
    Codec {
        name: "VP9 Video Extensions",
        package: "Microsoft.VP9VideoExtensions",
        probe: "vp9",
        store_ids: &["9N4D0MSV0403"],
    },
];

pub fn fourcc(codec: &str) -> Option<u32> {
    let tag: &[u8; 4] = match codec {
        "h265" => b"HEVC",
        "vp9" => b"VP90",
        "av1" => b"AV01",
        _ => return None,
    };
    Some(u32::from_le_bytes(*tag))
}

pub fn probe_decoder(codec: &str) -> Option<bool> {
    let tag = fourcc(codec)?;
    MediaFoundation::start()?.has_video_decoder(tag)
}

pub fn probe_all() -> Option<Vec<String>> {
    let mf = MediaFoundation::start()?;
    Some(
        EXTENSION_BACKED_VIDEO_CODECS
            .iter()
            .filter(|c| fourcc(c).and_then(|t| mf.has_video_decoder(t)) == Some(true))
            .map(|c| (*c).to_owned())
            .collect(),
    )
}

pub fn describe_probe(result: Option<&[String]>) -> String {
    match result {
        None => "unavailable".to_owned(),
        Some([]) => "none".to_owned(),
        Some(list) => list.join(","),
    }
}

pub type Probe = Arc<dyn Fn() -> Option<Vec<String>> + Send + Sync>;

pub struct CodecCapabilities {
    verified: Mutex<Option<Vec<String>>>,
    probe: Probe,
    log: Log,
}

impl CodecCapabilities {
    pub fn new(probe: Probe, log: Log) -> CodecCapabilities {
        CodecCapabilities {
            verified: Mutex::new(None),
            probe,
            log,
        }
    }

    pub fn system(log: Log) -> CodecCapabilities {
        CodecCapabilities::new(Arc::new(probe_all), log)
    }

    pub fn verified(&self) -> Option<Vec<String>> {
        self.verified.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn refresh(&self) {
        let result = (self.probe)();
        self.log.line(&format!(
            "[codec] capability probe: {}",
            describe_probe(result.as_deref())
        ));
        *self.verified.lock().unwrap_or_else(|e| e.into_inner()) = result;
    }
}

pub fn is_transient(failure_class: &str) -> bool {
    matches!(
        failure_class,
        FAIL_TIMEOUT | FAIL_UNVERIFIED_INSTALL | FAIL_UNKNOWN
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WingetResult {
    pub launched: bool,
    pub timed_out: bool,
    pub exit_code: i32,
    pub output: String,
}

impl WingetResult {
    pub fn not_launched() -> WingetResult {
        WingetResult {
            launched: false,
            timed_out: false,
            exit_code: -1,
            output: String::new(),
        }
    }

    pub fn timed_out() -> WingetResult {
        WingetResult {
            launched: true,
            timed_out: true,
            exit_code: -1,
            output: String::new(),
        }
    }

    pub fn exited(exit_code: i32, output: &str) -> WingetResult {
        WingetResult {
            launched: true,
            timed_out: false,
            exit_code,
            output: output.to_owned(),
        }
    }
}

pub fn classify_winget_failure(result: &WingetResult) -> &'static str {
    if !result.launched {
        return FAIL_WINGET_MISSING;
    }
    if result.timed_out {
        return FAIL_TIMEOUT;
    }
    let body = result.output.to_lowercase();
    if body.contains("no package found") {
        return FAIL_NOT_FOUND;
    }
    if ["purchase", "not owned", "entitlement"]
        .iter()
        .any(|t| body.contains(t))
    {
        return FAIL_NOT_ENTITLED;
    }
    if body.contains("source")
        && ["disabled", "agreement", "blocked"]
            .iter()
            .any(|t| body.contains(t))
    {
        return FAIL_SOURCE_UNAVAILABLE;
    }
    FAIL_UNKNOWN
}

mod iso_time {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serializer};
    use vrcr_shared::logutil::{iso8601_utc, parse_iso8601_utc};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&iso8601_utc(*t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let raw = Option::<String>::deserialize(d)?;
        Ok(raw.as_deref().and_then(parse_iso8601_utc).unwrap_or(UNIX_EPOCH))
    }
}

fn epoch() -> SystemTime {
    UNIX_EPOCH
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecEntry {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub failure_class: Option<String>,
    #[serde(with = "iso_time", default = "epoch")]
    pub last_attempt_utc: SystemTime,
    #[serde(rename = "package_family_name", default)]
    pub package: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecState {
    #[serde(default)]
    pub codecs: BTreeMap<String, CodecEntry>,
}

pub fn load_state(path: &Path) -> CodecState {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_state(path: &Path, state: &CodecState) {
    let Ok(json) = serde_json::to_vec(state) else {
        return;
    };
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".new");
    let tmp = PathBuf::from(tmp);
    if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecOutcome {
    pub codec: Codec,
    pub installed: bool,
    pub detail: &'static str,
}

pub trait CodecEnv {
    fn mf_probe(&self, codec: &str) -> bool;
    fn appx_probe(&self, package: &str) -> impl Future<Output = bool> + Send;
    fn winget_install(&self, store_id: &str) -> impl Future<Output = WingetResult> + Send;
    fn now(&self) -> SystemTime;
}

pub struct Installer<E> {
    env: E,
}

impl<E: CodecEnv> Installer<E> {
    pub fn new(env: E) -> Installer<E> {
        Installer { env }
    }

    pub async fn run(
        &self,
        codecs: &[Codec],
        state: &mut CodecState,
        mut report: impl FnMut(&CodecOutcome),
    ) -> bool {
        let mut dirty = false;
        for codec in codecs {
            let Some(key) = codec.store_ids.first() else {
                continue;
            };
            if self.should_skip(state, key) {
                continue;
            }
            let outcome = self.resolve_one(*codec).await;
            state.codecs.insert(
                (*key).to_owned(),
                CodecEntry {
                    status: if outcome.installed {
                        STATUS_INSTALLED
                    } else {
                        STATUS_FAILED
                    }
                    .to_owned(),
                    failure_class: (!outcome.installed).then(|| outcome.detail.to_owned()),
                    last_attempt_utc: self.env.now(),
                    package: codec.package.to_owned(),
                },
            );
            dirty = true;
            report(&outcome);
        }
        dirty
    }

    async fn resolve_one(&self, codec: Codec) -> CodecOutcome {
        let done = |installed: bool, detail: &'static str| CodecOutcome {
            codec,
            installed,
            detail,
        };
        if self.env.mf_probe(codec.probe) {
            return done(true, "mf");
        }
        if self.env.appx_probe(codec.package).await {
            return done(true, "appx");
        }
        let mut last = FAIL_UNKNOWN;
        for id in codec.store_ids {
            let result = self.env.winget_install(id).await;
            if result.launched && !result.timed_out && result.exit_code == 0 {
                if self.env.mf_probe(codec.probe) {
                    return done(true, "winget+mf");
                }
                if self.env.appx_probe(codec.package).await {
                    return done(true, "winget+appx");
                }
                last = FAIL_UNVERIFIED_INSTALL;
                continue;
            }
            last = classify_winget_failure(&result);
            if last == FAIL_WINGET_MISSING {
                break;
            }
        }
        done(false, last)
    }

    pub fn should_skip(&self, state: &CodecState, key: &str) -> bool {
        let Some(entry) = state.codecs.get(key) else {
            return false;
        };
        match entry.status.as_str() {
            STATUS_INSTALLED => true,
            STATUS_FAILED => {
                let window = if is_transient(entry.failure_class.as_deref().unwrap_or(FAIL_UNKNOWN)) {
                    TRANSIENT_RETRY_WINDOW
                } else {
                    PERMANENT_RETRY_WINDOW
                };
                self.env
                    .now()
                    .duration_since(entry.last_attempt_utc)
                    .map_or(true, |elapsed| elapsed < window)
            }
            _ => false,
        }
    }
}

fn hidden(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

async fn read_all(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> String {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_end(&mut buf).await;
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn truncate_bytes(mut text: String, cap: usize) -> String {
    if text.len() > cap {
        let mut at = cap;
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        text.truncate(at);
    }
    text
}

pub struct SystemEnv {
    pub log: Log,
}

impl CodecEnv for SystemEnv {
    fn mf_probe(&self, codec: &str) -> bool {
        probe_decoder(codec) == Some(true)
    }

    async fn appx_probe(&self, package: &str) -> bool {
        let mut cmd = hidden("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("Get-AppxPackage -Name '{package}' | Select-Object -ExpandProperty Name"),
        ]);
        let Ok(mut child) = cmd.spawn() else {
            return false;
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let run = async {
            let (out, _) = tokio::join!(read_all(stdout), read_all(stderr));
            let _ = child.wait().await;
            out
        };
        match tokio::time::timeout(APPX_TIMEOUT, run).await {
            Ok(out) => !out.trim().is_empty(),
            Err(_) => false,
        }
    }

    async fn winget_install(&self, store_id: &str) -> WingetResult {
        let mut cmd = hidden("winget");
        cmd.args([
            "install",
            "--id",
            store_id,
            "--source",
            "msstore",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--silent",
        ]);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return WingetResult::not_launched(),
            Err(e) => return WingetResult::exited(-1, &format!("{:?}", e.kind())),
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let run = async {
            let (out, err) = tokio::join!(read_all(stdout), read_all(stderr));
            let status = child.wait().await;
            (out, err, status)
        };
        let Ok((out, err, status)) = tokio::time::timeout(PER_CODEC_TIMEOUT, run).await else {
            return WingetResult::timed_out();
        };
        let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
        let output = truncate_bytes(format!("{out}\n{err}"), WINGET_OUTPUT_CAP);
        self.log.line(&format!(
            "[codec] winget {store_id} exit={code} out={}",
            sanitize_for_console(&output, 400)
        ));
        WingetResult::exited(code, &output)
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

pub fn report_message(outcome: &CodecOutcome) -> (bool, String) {
    if outcome.installed {
        (
            true,
            format!("{} ready ({}).", outcome.codec.name, outcome.detail),
        )
    } else {
        let when = if is_transient(outcome.detail) {
            "tomorrow"
        } else {
            "next week"
        };
        (
            false,
            format!(
                "{} install failed ({}); will retry {when}.",
                outcome.codec.name, outcome.detail
            ),
        )
    }
}

pub async fn run_check<E: CodecEnv>(
    installer: &Installer<E>,
    state_path: &Path,
    capabilities: &CodecCapabilities,
    ux: &Ux,
) {
    let mut state = load_state(state_path);
    let dirty = installer
        .run(&REQUIRED, &mut state, |outcome| {
            let (ok, text) = report_message(outcome);
            if ok {
                ux.success(Component::Codec, &text);
                capabilities.refresh();
            } else {
                ux.warn(Component::Codec, &text);
            }
        })
        .await;
    if dirty {
        save_state(state_path, &state);
    }
}

pub fn start_background_check(
    enabled: bool,
    state_dir: &Path,
    capabilities: Arc<CodecCapabilities>,
    ux: Ux,
    log: Log,
) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled {
        log.line("[codec] auto-install disabled by settings");
        return None;
    }
    let _ = fs::create_dir_all(state_dir);
    let path = state_dir.join(STATE_FILE);
    Some(tokio::spawn(async move {
        let installer = Installer::new(SystemEnv { log });
        run_check(&installer, &path, &capabilities, &ux).await;
    }))
}
