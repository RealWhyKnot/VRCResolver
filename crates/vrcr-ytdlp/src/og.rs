use std::future::Future;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

pub const TIMEOUT_EXIT_CODE: i32 = 1;
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(5);
const MAX_STDOUT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 1024 * 1024;

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
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let result = collect_output(stdout, stderr, child.wait(), budget).await;
    if result.as_ref().map_or(true, |output| output.timed_out) {
        kill_tree(&mut child).await;
    }
    result
}

async fn read_output(reader: Option<impl AsyncRead + Unpin>, limit: u64) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    if let Some(reader) = reader {
        reader.take(limit + 1).read_to_end(&mut output).await?;
        if u64::try_from(output.len()).unwrap_or(u64::MAX) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "yt-dlp output exceeds the size limit",
            ));
        }
    }
    Ok(output)
}

async fn collect_output(
    stdout: Option<impl AsyncRead + Unpin>,
    stderr: Option<impl AsyncRead + Unpin>,
    wait: impl Future<Output = io::Result<ExitStatus>>,
    budget: Duration,
) -> io::Result<OgRun> {
    let readers = async {
        tokio::try_join!(
            read_output(stdout, MAX_STDOUT_BYTES),
            read_output(stderr, MAX_STDERR_BYTES)
        )
    };
    let wait = tokio::time::timeout(budget, wait);
    tokio::pin!(readers, wait);
    let mut captured = None;
    let status = tokio::select! {
        status = &mut wait => status,
        output = &mut readers => {
            captured = Some(output?);
            wait.await
        }
    };
    match status {
        Ok(status) => {
            let status = status?;
            let (stdout, stderr) = match captured {
                Some(output) => output,
                None => tokio::time::timeout(DRAIN_AFTER_EXIT, readers)
                    .await
                    .unwrap_or_else(|_| Ok((Vec::new(), Vec::new())))?,
            };
            Ok(OgRun {
                exit: status.code().unwrap_or(-1),
                timed_out: false,
                stdout,
                stderr,
            })
        }
        Err(_) => Ok(OgRun {
            exit: TIMEOUT_EXIT_CODE,
            timed_out: true,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }),
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
            .kill_on_drop(true)
            .creation_flags(0x0800_0000);
        let _ = tokio::time::timeout(Duration::from_secs(5), taskkill.status()).await;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use std::future::{pending, ready};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    use super::*;

    struct HeldReader(Arc<AtomicUsize>);

    impl AsyncRead for HeldReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl Drop for HeldReader {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn success() -> ExitStatus {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }

    #[tokio::test]
    async fn oversized_output_is_rejected_without_reading_the_rest() {
        let mut source: &[u8] = b"123456789";
        let result = read_output(Some(&mut source), 4).await;
        assert_eq!(
            result.expect_err("oversized output must fail").kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(source, b"6789");
        assert_eq!(
            read_output(Some(&b"1234"[..]), 4)
                .await
                .expect("exact limit is allowed"),
            b"1234"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn drain_timeout_drops_both_pipe_readers() {
        let closed = Arc::new(AtomicUsize::new(0));
        let output = collect_output(
            Some(HeldReader(Arc::clone(&closed))),
            Some(HeldReader(Arc::clone(&closed))),
            ready(Ok(success())),
            Duration::from_secs(30),
        )
        .await
        .expect("the child exited successfully");
        assert_eq!(output.exit, 0);
        assert_eq!(closed.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn process_timeout_drops_both_pipe_readers() {
        let closed = Arc::new(AtomicUsize::new(0));
        let output = collect_output(
            Some(HeldReader(Arc::clone(&closed))),
            Some(HeldReader(Arc::clone(&closed))),
            pending(),
            Duration::from_secs(30),
        )
        .await
        .expect("timeout is reported in the result");
        assert!(output.timed_out);
        assert_eq!(closed.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_capture_drops_both_pipe_readers() {
        let closed = Arc::new(AtomicUsize::new(0));
        let capture = collect_output(
            Some(HeldReader(Arc::clone(&closed))),
            Some(HeldReader(Arc::clone(&closed))),
            pending(),
            Duration::from_secs(30),
        );
        {
            tokio::pin!(capture);
            tokio::select! {
                biased;
                _ = &mut capture => panic!("capture must remain pending"),
                () = ready(()) => {}
            }
        }
        assert_eq!(closed.load(Ordering::SeqCst), 2);
    }
}
