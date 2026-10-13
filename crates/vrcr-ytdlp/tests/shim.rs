#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::mpsc;
use vrcr_shared::wire::{Player, og_reason};
use vrcr_ytdlp::og;
use vrcr_ytdlp::pipe;
use vrcr_ytdlp::shim::{Shim, ShimConfig};

const URL: &str = "https://example.com/watch";
const RESOLVED: &str = "https://media.example.com/stream/video.m3u8";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Rig {
    dir: TempDir,
    pipe_name: String,
    stdout: Capture,
}

impl Rig {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            pipe_name: format!("vrcresolver.test.{}", uuid::Uuid::new_v4().simple()),
            stdout: Capture::default(),
        }
    }

    fn shim(&self) -> Shim {
        let cfg = ShimConfig {
            pipe_name: self.pipe_name.clone(),
            state_root: self.dir.path().join("state"),
            exe_dir: self.dir.path().join("exe"),
            vrc_tools_dir: None,
            log_path: Some(self.log_path()),
            version: "test".to_owned(),
        };
        Shim::new(cfg, Box::new(self.stdout.clone()), Box::new(Capture::default()))
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("yt-dlp-wrapper.log")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.log_path()).unwrap_or_default()
    }
}

struct StubPipe {
    lines: mpsc::UnboundedReceiver<String>,
    task: tokio::task::JoinHandle<()>,
}

impl StubPipe {
    fn start(name: &str, reply: Option<&str>, delay: Duration) -> Self {
        let full = pipe::full_name(name);
        let reply = reply.map(|r| format!("{r}\n"));
        let (tx, lines) = mpsc::unbounded_channel();
        let first = delay
            .is_zero()
            .then(|| ServerOptions::new().create(&full).unwrap());
        let task = tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let mut server = first.unwrap_or_else(|| ServerOptions::new().create(&full).unwrap());
            loop {
                if server.connect().await.is_err() {
                    return;
                }
                let next = ServerOptions::new().create(&full).unwrap();
                let mut conn = std::mem::replace(&mut server, next);
                let reply = reply.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut byte = [0u8; 1];
                    while conn.read(&mut byte).await.is_ok_and(|n| n == 1) && byte[0] != b'\n' {
                        buf.push(byte[0]);
                    }
                    let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
                    if let Some(r) = reply {
                        let _ = conn.write_all(r.as_bytes()).await;
                        let _ = conn.flush().await;
                    }
                    let mut rest = Vec::new();
                    let _ = conn.read_to_end(&mut rest).await;
                });
            }
        });
        Self { lines, task }
    }

    async fn next_line(&mut self) -> serde_json::Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.recv())
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

impl Drop for StubPipe {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn resolved_line(url: &str) -> String {
    format!(r#"{{"action":"resolved","id":"srv1","url":"{url}"}}"#)
}

#[tokio::test]
async fn resolve_over_pipe_returns_the_resolved_url() {
    let rig = Rig::new();
    let _server = StubPipe::start(&rig.pipe_name, Some(&resolved_line(RESOLVED)), Duration::ZERO);
    let out = rig
        .shim()
        .resolve_over_pipe(URL, Player::AvPro, None, None, false)
        .await;
    assert_eq!(out.url.as_deref(), Some(RESOLVED));
    assert_eq!(out.fallback_reason, None);
    assert_eq!(out.public_message, None);
}

#[tokio::test]
async fn resolve_over_pipe_returns_the_server_fallback_reason() {
    let rig = Rig::new();
    let reply = r#"{"action":"fallback_native","id":"srv1","reason":"resolve_failed","public_message":"Could not resolve this link"}"#;
    let _server = StubPipe::start(&rig.pipe_name, Some(reply), Duration::ZERO);
    let out = rig
        .shim()
        .resolve_over_pipe(URL, Player::AvPro, None, None, false)
        .await;
    assert_eq!(out.url, None);
    assert_eq!(out.fallback_reason, Some(og_reason::SERVER_FALLBACK_NATIVE));
    assert_eq!(out.public_message.as_deref(), Some("Could not resolve this link"));
}

#[tokio::test]
async fn resolve_over_pipe_rejects_avpro_incompatible_urls() {
    let rig = Rig::new();
    let line = resolved_line("rtmp://live.example.com/stream");
    let _server = StubPipe::start(&rig.pipe_name, Some(&line), Duration::ZERO);
    let out = rig
        .shim()
        .resolve_over_pipe(URL, Player::AvPro, None, None, false)
        .await;
    assert_eq!(out.url, None);
    assert_eq!(out.fallback_reason, Some(og_reason::AVPRO_INCOMPATIBLE));
}

#[tokio::test]
async fn resolve_over_pipe_sends_a_wrapper_deadline_below_its_budget() {
    let rig = Rig::new();
    let mut server = StubPipe::start(&rig.pipe_name, Some(&resolved_line(RESOLVED)), Duration::ZERO);
    let fmt = "(mp4/best)[height<=?720]";
    rig.shim()
        .resolve_over_pipe(
            URL,
            Player::Unity,
            Some(fmt),
            Some(Duration::from_secs(10)),
            false,
        )
        .await;
    let req = server.next_line().await;
    assert_eq!(req["action"], "resolve");
    assert_eq!(req["player"], "unity");
    assert_eq!(req["maxHeight"], 720);
    assert_eq!(req["vrchat_format_arg"], fmt);
    let deadline = req["wrapper_deadline_ms"].as_i64().unwrap();
    assert!((8001..=9000).contains(&deadline), "{deadline}");
}

#[tokio::test]
async fn notifies_return_at_once_after_the_pipe_connect_failed() {
    let rig = Rig::new();
    let mut shim = rig.shim();
    let out = shim
        .resolve_over_pipe(URL, Player::AvPro, None, Some(Duration::from_secs(2)), false)
        .await;
    assert_eq!(out.fallback_reason, Some(og_reason::PIPE_CONNECT_FAILED));
    let started = Instant::now();
    shim.notify_og_fallback(Some(URL), og_reason::PIPE_CONNECT_FAILED, 2000, None)
        .await;
    shim.notify_og_failed(URL, "timeout", 1, String::new(), 2000)
        .await;
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn notify_reaches_the_watchdog_while_the_pipe_is_up() {
    let rig = Rig::new();
    let mut server = StubPipe::start(&rig.pipe_name, None, Duration::ZERO);
    rig.shim()
        .notify_og_fallback(
            Some(URL),
            og_reason::PIPE_RESOLVE_FAILED,
            123,
            Some("Could not resolve this link".to_owned()),
        )
        .await;
    let frame = server.next_line().await;
    assert_eq!(frame["action"], "og_fallback_notify");
    assert_eq!(frame["reason"], og_reason::PIPE_RESOLVE_FAILED);
    assert_eq!(frame["elapsed_ms"], 123);
    assert_eq!(frame["public_message"], "Could not resolve this link");
}

#[tokio::test]
async fn connect_retry_reaches_a_late_watchdog_and_notifies_still_go_out() {
    let rig = Rig::new();
    let reply = r#"{"action":"fallback_native","id":"srv1","reason":"resolve_failed"}"#;
    let mut server = StubPipe::start(&rig.pipe_name, Some(reply), Duration::from_millis(1400));
    let mut shim = rig.shim();
    let out = shim
        .resolve_over_pipe(URL, Player::AvPro, None, None, false)
        .await;
    assert_eq!(out.fallback_reason, Some(og_reason::SERVER_FALLBACK_NATIVE));
    server.next_line().await;
    shim.notify_og_fallback(Some(URL), og_reason::SERVER_FALLBACK_NATIVE, 2000, None)
        .await;
    assert_eq!(server.next_line().await["action"], "og_fallback_notify");
    assert!(rig.log().contains("pipe reconnect OK retry=1"));
}

#[tokio::test]
async fn run_prints_exactly_the_resolved_url_and_exits_zero() {
    let rig = Rig::new();
    let line = resolved_line(&format!("  {RESOLVED}  "));
    let _server = StubPipe::start(&rig.pipe_name, Some(&line), Duration::ZERO);
    let args: Vec<String> = ["--no-check-certificate", "-f", "best[height<=1080]", URL]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let code = rig.shim().run(&args).await;
    assert_eq!(code, 0);
    assert_eq!(rig.stdout.text(), format!("{RESOLVED}\n"));
    let log = rig.log();
    assert!(log.contains("END exit=0 outcome=pipe-resolved"), "{log}");
}

#[tokio::test]
async fn run_passes_first_party_playback_urls_straight_through_without_a_relay() {
    let rig = Rig::new();
    let direct = "https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc";
    let code = rig.shim().run(&[direct.to_owned()]).await;
    assert_eq!(code, 0);
    assert_eq!(rig.stdout.text(), format!("{direct}\n"));
    assert!(rig.log().contains("outcome=direct-first-party-playback "));
}

#[tokio::test]
async fn run_wraps_first_party_playback_urls_when_the_relay_is_listening() {
    let rig = Rig::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = rig.dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("relay_port.txt"), port.to_string()).unwrap();
    let direct = "https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc";
    let code = rig.shim().run(&[direct.to_owned()]).await;
    assert_eq!(code, 0);
    let out = rig.stdout.text();
    assert!(
        out.starts_with(&format!("http://localhost.youtube.com:{port}/play/")),
        "{out}"
    );
    assert!(out.ends_with(".m3u8?target=") || out.contains("manifest.m3u8?target="));
    assert_eq!(
        vrcr_shared::trust_gateway::extract_target(out.trim()).as_deref(),
        Some(direct)
    );
}

#[tokio::test]
async fn run_rejects_a_resolved_url_that_points_at_the_local_network() {
    let rig = Rig::new();
    let line = resolved_line("http://192.168.1.1/video.mp4");
    let mut server = StubPipe::start(&rig.pipe_name, Some(&line), Duration::ZERO);
    let code = rig.shim().run(&[URL.to_owned()]).await;
    assert_eq!(code, 1);
    assert_eq!(rig.stdout.text(), "");
    server.next_line().await;
    let notify = server.next_line().await;
    assert_eq!(notify["action"], "og_fallback_notify");
    assert_eq!(notify["reason"], og_reason::RESOLVED_URL_REJECTED);
    let log = rig.log();
    assert!(log.contains("FALLBACK no-og"), "{log}");
    assert!(log.contains("terminal_failure "), "{log}");
}

fn cmd() -> &'static Path {
    Path::new("cmd.exe")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[tokio::test]
async fn og_run_collects_output_and_exit_code() {
    let dir = std::env::temp_dir();
    let run = og::run(
        cmd(),
        &strings(&["/c", "echo hello& echo oops 1>&2& exit 3"]),
        &dir,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(run.exit, 3);
    assert!(!run.timed_out);
    assert!(String::from_utf8_lossy(&run.stdout).contains("hello"));
    assert!(String::from_utf8_lossy(&run.stderr).contains("oops"));
}

#[tokio::test]
async fn og_run_kills_the_tree_when_the_budget_runs_out() {
    let dir = std::env::temp_dir();
    let started = Instant::now();
    let run = og::run(
        cmd(),
        &strings(&["/c", "ping -n 30 127.0.0.1 >nul"]),
        &dir,
        Duration::from_millis(500),
    )
    .await
    .unwrap();
    assert!(run.timed_out);
    assert_eq!(run.exit, og::TIMEOUT_EXIT_CODE);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn og_failures_classify_into_fixed_reasons() {
    let cases = [
        ("ERROR: HTTP Error 403: Forbidden", "cf_403"),
        ("ERROR: HTTP Error 429: Too Many Requests", "rate_limited"),
        ("Sign in to confirm you're not a bot", "sign_in_required"),
        ("ERROR: [youtube] x: Video unavailable", "content_not_found"),
        (
            "ERROR: Private video. Sign in if you've been granted access",
            "content_not_found",
        ),
        ("HTTP Error 404: Not Found", "content_not_found"),
        ("something else", "unknown"),
        ("", "unknown"),
    ];
    for (stderr, expected) in cases {
        assert_eq!(og::classify_failure(stderr), expected, "{stderr}");
    }
}

#[test]
fn the_shim_binary_carries_the_wrapper_marker_and_identity() {
    let exe = Path::new(env!("CARGO_BIN_EXE_yt-dlp"));
    assert!(vrcr_shared::wrapper_identity::contains_marker(exe));
    assert_eq!(
        vrcr_shared::wrapper_identity::classify(exe, None),
        vrcr_shared::wrapper_identity::WrapperKind::Ours
    );
}
