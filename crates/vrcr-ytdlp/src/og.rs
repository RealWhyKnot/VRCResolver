use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub const TIMEOUT_EXIT_CODE: i32 = 1;
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(5);

const CONTENT_GONE: [&str; 9] = [
    "Video unavailable",
    "This video is no longer available",
    "This video is not available",
    "removed by the uploader",
    "closed their YouTube account",
    "account associated",
    "Private video",
    "HTTP Error 404",
    "HTTP Error 410",
];

pub fn classify_failure(stderr: &str) -> &'static str {
    if stderr.is_empty() {
        return "unknown";
    }
    let lower = stderr.to_lowercase();
    let has = |needle: &str| lower.contains(&needle.to_lowercase());
    if has("HTTP Error 403") {
        "cf_403"
    } else if has("HTTP Error 429") {
        "rate_limited"
    } else if has("Sign in to confirm") || has("Sign in to verify") {
        "sign_in_required"
    } else if CONTENT_GONE.iter().any(|n| has(n)) {
        "content_not_found"
    } else {
        "unknown"
    }
}

pub struct OgRun {
    pub exit: i32,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub async fn run(
    og_path: &Path,
    args: &[String],
    working_dir: &Path,
    budget: Duration,
) -> std::io::Result<OgRun> {
    let mut cmd = Command::new(og_path);
    cmd.args(args)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn()?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let read_out = async move {
        let mut buf = Vec::new();
        if let Some(s) = out.as_mut() {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    };
    let read_err = async move {
        let mut buf = Vec::new();
        if let Some(s) = err.as_mut() {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    };
    let readers = tokio::spawn(async move { tokio::join!(read_out, read_err) });
    match tokio::time::timeout(budget, child.wait()).await {
        Ok(status) => {
            let status = status?;
            let (stdout, stderr) = match tokio::time::timeout(DRAIN_AFTER_EXIT, readers).await {
                Ok(joined) => joined.unwrap_or_default(),
                Err(_) => (Vec::new(), Vec::new()),
            };
            Ok(OgRun {
                exit: status.code().unwrap_or(-1),
                timed_out: false,
                stdout,
                stderr,
            })
        }
        Err(_) => {
            kill_tree(&mut child).await;
            readers.abort();
            Ok(OgRun {
                exit: TIMEOUT_EXIT_CODE,
                timed_out: true,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }
}

async fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut taskkill = Command::new("taskkill");
        taskkill
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000);
        let _ = tokio::time::timeout(Duration::from_secs(5), taskkill.status()).await;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}
