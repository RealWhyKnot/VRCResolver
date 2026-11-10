use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_shared::wire::{Player, ResolveRequest, ResolveResponse};
use vrcr_watchdog::diagnostics::{PlaybackLog, player_error, startup};
use vrcr_watchdog::relay::hitch::TimingSample;

use crate::common::mesh::Capture;

const SOURCE: &str = "https://source.example/private/title?username=secret";
const TARGET: &str =
    "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_fixture/index.m3u8?token=secret";

fn now(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn output() -> (Ux, Capture) {
    let capture = Capture::default();
    (
        Ux::new(Box::new(capture.clone()), Style::plain(), String::new),
        capture,
    )
}

fn resolve(playback: &PlaybackLog, ux: &Ux) {
    let request = ResolveRequest {
        id: "resolve-one".to_owned(),
        url: SOURCE.to_owned(),
        player: Some(Player::AvPro),
        ..Default::default()
    };
    playback.resolving(&request, now(1), ux);
    playback.resolved(
        &request,
        &ResolveResponse {
            action: "resolved".to_owned(),
            id: "resolve-one".to_owned(),
            url: Some(TARGET.to_owned()),
            engine: Some("yt-dlp".to_owned()),
            config: Some("fixture".to_owned()),
            protocol: Some("hls".to_owned()),
            container: Some("mpegts".to_owned()),
            video_codec: Some("h264".to_owned()),
            audio_codec: Some("aac".to_owned()),
            resolved_height: Some(1080),
            ..Default::default()
        },
        vrcr_watchdog::diagnostics::ResolveDelivery {
            cached: true,
            retried: false,
            bytes: 200,
        },
        now(2),
        ux,
    );
}

fn sample(status: u16) -> TimingSample {
    TimingSample {
        suppressed: false,
        recovery: false,
        retry_reason: None,
        request_id: "relay-one".to_owned(),
        stream_id: "wk_fixture".to_owned(),
        player_status: status,
        playlist_shape: None,
        method: "GET".to_owned(),
        local_path: "/play/player-one/index.m3u8".to_owned(),
        target_url: TARGET.to_owned(),
        status,
        header_ms: 10,
        total_ms: 25,
        bytes_out: 100,
        media: None,
        lazy_state: None,
        lazy_wait_ms: -1,
        lazy_generator: None,
        failure: None,
    }
}

fn private_text_is_absent(text: &str) {
    for private in [
        "private", "title", "username", "secret", "?", "/api/", "\u{1b}",
    ] {
        assert!(!text.contains(private), "{text}");
    }
    assert!(text.is_ascii());
}

#[test]
fn diagnostics_resolve_playlist_and_closing_session_share_one_playback() {
    let playback = PlaybackLog::default();
    let (ux, capture) = output();
    resolve(&playback, &ux);
    let mut request = sample(200);
    request.playlist_shape = Some("kind=media mode=live segments=2880 target_ms=5000 bytes_in=5600000 bytes_out=31000 trimmed=true kept=12".to_owned());
    for _ in 0..50 {
        playback.request(&request, "", now(3), &ux);
    }
    let mut segment = sample(503);
    segment.local_path = "/play/player-one/seg_000001.ts".to_owned();
    segment.target_url = TARGET.replace("index.m3u8", "seg_000001.ts");
    segment.total_ms = 3000;
    playback.request(&segment, "segment-http-503,slow-segment-total", now(4), &ux);
    let context = playback.context(SOURCE).expect("resolve context");
    assert_eq!(context.request_id, "relay-one");
    assert_eq!(context.stream_id, "wk_fixture");
    assert_eq!(context.source, "server");
    playback.close(TARGET, "player_closed", now(5), &ux);
    let text = capture.text();
    assert!(
        text.contains("resolve start req=resolve-one host=source.example player=avpro"),
        "{text}"
    );
    assert!(text.contains("resolve done req=resolve-one stream=wk_fixture host=source.example action=resolved engine=yt-dlp config=fixture protocol=hls container=mpegts video=h264 audio=aac height=1080 elapsed_ms=1000 cache=true"), "{text}");
    assert_eq!(text.matches("playlist req=").count(), 1);
    assert!(text.contains("kind=media mode=live segments=2880 target_ms=5000 bytes_in=5600000 bytes_out=31000 trimmed=true kept=12"));
    assert!(text.contains("status=503 elapsed_ms=3000 bytes=100 retry=false sent=503 reason=upstream_status hitches=segment-http-503,slow-segment-total body=complete count=1"));
    assert!(text.contains("session playback=resolve-one host=source.example duration_ms=4000 manifests=50 segments=1 other=0 bytes=5100 slowest_manifest_ms=25 slowest_segment_ms=3000 hitches=segment-http-503:1,slow-segment-total:1 retries=0 statuses=503:1 ended=player_closed"), "{text}");
    private_text_is_absent(&text);
}

#[test]
fn diagnostics_repeats_flush_exact_counts_and_preserve_active_playbacks() {
    let playback = PlaybackLog::default();
    let (ux, capture) = output();
    let mut request = sample(404);
    for index in 0..500 {
        request.request_id = format!("relay-{index}");
        playback.request(&request, "manifest-http-404", now(index / 5), &ux);
    }
    playback.close(TARGET, "error", now(101), &ux);
    let text = capture.text();
    assert_eq!(text.matches("request failed").count(), 3, "{text}");
    assert!(text.contains("count=301"));
    assert!(text.contains("count=500"));
    assert!(
        text.contains("hitches=manifest-http-404:500 retries=499 statuses=404:500 ended=error"),
        "{text}"
    );
    let (ux, capture) = output();
    playback.begin_request(
        "/play/active/file.mp4",
        TARGET,
        &request.request_id,
        now(1),
        &ux,
    );
    playback.expire(now(999), &ux);
    assert_eq!(capture.text(), "");
    playback.close(TARGET, "replaced", now(1000), &ux);
    assert_eq!(capture.text(), "");
    request.local_path = "/play/active/file.mp4".to_owned();
    playback.request(&request, "manifest-http-404", now(1001), &ux);
    assert!(capture.text().contains("ended=replaced"));
}

#[test]
fn diagnostics_startup_evidence_and_console_style_are_explicit() {
    let directory = TempDir::new().expect("state");
    assert_eq!(startup::begin(directory.path()), "unknown");
    assert_eq!(startup::begin(directory.path()), "unclean_unknown");
    startup::record(directory.path(), "panic_observed");
    assert_eq!(startup::begin(directory.path()), "panic_observed");
    startup::record(directory.path(), "shutdown-quit");
    assert_eq!(startup::begin(directory.path()), "shutdown-quit");
    let line = startup::line(&startup::Environment {
        version: "1.2.3",
        os: "Windows 10.0.26200",
        server: "us1.vrcresolver.com",
        port: 12345,
        tls: "ready",
        hosts: "present",
        vrchat: true,
        previous: "unclean_unknown",
    });
    assert_eq!(
        line,
        "environment version=1.2.3 os=Windows_10.0.26200 server=us1.vrcresolver.com relay_port=12345 tls=ready hosts=present vrchat=running player=not_seen previous=unclean_unknown"
    );
    for redirected in [false, true] {
        let style = startup::console_style(redirected, Some("1"));
        assert!(!style.color);
        assert!(!style.unicode);
    }
}

#[test]
fn diagnostics_feedback_is_bounded_and_has_only_technical_fields() {
    let context = vrcr_watchdog::diagnostics::FailureContext {
        playback_id: "resolve-one".to_owned(),
        request_id: "relay-one".to_owned(),
        stream_id: "wk_fixture".to_owned(),
        source: "server".to_owned(),
        upstream_status: 503,
        ..Default::default()
    };
    let json = vrcr_watchdog::mesh::frames::playback_feedback_payload(
        vrcr_watchdog::mesh::frames::PlaybackFeedback {
            url: "https://media.example/file",
            kind: "load_failure",
            ms_since_open: 12500,
            client_id: "existing-client",
            correlation_id: Some("existing-correlation"),
            timestamp: now(1),
            delivered_height: None,
            detail: Some(
                "[AVProVideo] Error: Loading failed ErrorCode: 0x80004005 private title username=secret",
            ),
            context: Some(&context),
        },
    );
    let value: serde_json::Value = serde_json::from_str(&json).expect("feedback");
    assert_eq!(value["request_id"], "relay-one");
    assert_eq!(value["stream_id"], "wk_fixture");
    assert_eq!(value["failure_source"], "server");
    assert_eq!(value["ms_since_open"], 12500);
    assert_eq!(value["player_error"], "loading_failed,code=0x80004005");
    assert!(value["detail"].as_str().expect("detail").len() <= 64);
    assert_eq!(
        value["detail"],
        "source=server status=503 error=loading_failed,code=0x80004005"
    );
    assert!(!json.contains("private"));
    assert!(!json.contains("secret"));
    assert_eq!(
        player_error("unknown message about a private title"),
        "player_error"
    );
}

#[test]
fn diagnostics_monitor_closes_after_a_short_simulated_playback() {
    use std::sync::Mutex;
    use vrcr_watchdog::monitoring::vrclog::{MonitorDeps, PlaybackSink, VrcLogMonitor};
    #[derive(Default)]
    struct Sink(Mutex<Vec<vrcr_watchdog::ipc::Feedback>>);
    impl PlaybackSink for Sink {
        fn send_playback_feedback(&self, feedback: vrcr_watchdog::ipc::Feedback) {
            self.0.lock().expect("feedback").push(feedback);
        }
    }
    let (ux, capture) = output();
    let stats = Arc::new(vrcr_watchdog::stats::Stats::new(now(1)));
    resolve(&stats.playback, &ux);
    let sink = Arc::new(Sink::default());
    let mut deps = MonitorDeps::new(
        Arc::<Sink>::clone(&sink),
        Arc::new(RwLock::new(FirstPartyPolicy::default())),
        ux,
        Log::disabled(),
    );
    deps.stats = Some(Arc::clone(&stats));
    let directory = TempDir::new().expect("logs");
    let mut monitor = VrcLogMonitor::new(deps, directory.path().to_path_buf());
    monitor.process_text(&format!("[AVProVideo] Opening {TARGET}\n"), now(2));
    monitor.process_text(
        "[AVProVideo] Error: Loading failed ErrorCode: 0x80004005 private title\n",
        now(4),
    );
    let messages = sink.0.lock().expect("feedback");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].ms_since_open, 2000);
    assert_eq!(
        messages[0].detail.as_deref(),
        Some("loading_failed,code=0x80004005")
    );
    drop(messages);
    assert!(capture.text().contains("ended=error"));
    assert!(
        capture
            .text()
            .contains("error=loading_failed,code=0x80004005")
    );
    assert!(!capture.text().contains("[warn]"));
    private_text_is_absent(&capture.text());
}

#[test]
fn diagnostics_perf_counts_hitches_before_repeat_suppression() {
    let stats = vrcr_watchdog::stats::Stats::default();
    stats.perf.enable();
    for _ in 0..500 {
        stats
            .perf
            .record_relay(vrcr_watchdog::stats::RelayKind::Manifest, true, 10, 20, 100);
        stats.perf.record_diagnostics("manifest-http-404", 404);
    }
    let line = vrcr_watchdog::monitoring::perf::perf_line(
        Duration::from_secs(60),
        &mut stats.perf.take(),
        None,
        None,
    );
    assert!(line.starts_with("[perf] window=60s relay_req=500 relay_err=500"));
    assert!(line.contains("hitches=manifest-http-404:500 statuses=404:500"));
    assert!(!line.contains("manifest_ms="));
}

#[test]
fn diagnostics_server_messages_keep_categories_and_drop_private_text() {
    let line = vrcr_watchdog::mesh::frames::resolve_log_line(
        &serde_json::json!({"id":"request-one", "level":"warn", "message":"request timeout for https://private.example/title?secret=user"}),
    );
    assert_eq!(
        line,
        "[mesh][resolve_log] id=request-one level=warn reason=timeout"
    );
    let line = vrcr_watchdog::mesh::frames::resolve_log_line(
        &serde_json::json!({"id":"request-one", "message":"trying fixture"}),
    );
    assert_eq!(
        line,
        "[mesh][resolve_log] id=request-one level=info phase=trying config=fixture"
    );
    assert_eq!(
        vrcr_watchdog::diagnostics::error_kind(
            "certificate error for https://private.example/title"
        ),
        "tls_error"
    );
}

#[tokio::test]
async fn diagnostics_playlist_shapes_cover_master_vod_and_invalid_bodies() {
    use bytes::Bytes;
    use vrcr_watchdog::relay::manifest::{MAX_MANIFEST_BYTES, localize_relay_stream};
    for (body, expected) in [
        (
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nindex.m3u8\n",
            "kind=master mode=unknown segments=0 target_ms=0",
        ),
        (
            "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:5,\na.ts\n#EXT-X-ENDLIST\n",
            "kind=media mode=vod segments=1 target_ms=5000",
        ),
        (
            "invalid body",
            "kind=unknown mode=unknown segments=0 target_ms=0",
        ),
    ] {
        let input = futures_util::stream::iter([Ok(Bytes::from(body))]);
        let mut output = Vec::new();
        let result = localize_relay_stream(
            &FirstPartyPolicy::default(),
            input,
            &mut output,
            MAX_MANIFEST_BYTES,
        )
        .await
        .expect("manifest");
        let shape = result.shape();
        assert!(shape.starts_with(expected), "{shape}");
        assert!(
            shape.contains(&format!(
                "bytes_in={} bytes_out={}",
                body.len(),
                output.len()
            )),
            "{shape}"
        );
        assert!(shape.contains("trimmed=false"));
    }
}

#[test]
fn diagnostics_pending_repeats_flush_while_playback_is_still_open() {
    let playback = PlaybackLog::default();
    let (ux, capture) = output();
    let request = sample(502);
    playback.request(&request, "manifest-http-502", now(1), &ux);
    playback.request(&request, "manifest-http-502", now(2), &ux);
    assert_eq!(capture.text().matches("request failed").count(), 1);
    playback.expire(now(61), &ux);
    assert_eq!(capture.text().matches("request failed").count(), 2);
    assert!(capture.text().contains("count=2"));
    assert!(!capture.text().contains("session playback="));
    playback.close(TARGET, "player_closed", now(62), &ux);
    assert_eq!(capture.text().matches("request failed").count(), 2);
    assert!(
        capture
            .text()
            .contains("statuses=502:2 ended=player_closed")
    );
}

#[test]
fn diagnostics_resolve_failure_has_a_disposition_and_closes_only_its_playback() {
    let playback = PlaybackLog::default();
    let (ux, capture) = output();
    let request = ResolveRequest {
        id: "failed-one".to_owned(),
        url: SOURCE.to_owned(),
        ..Default::default()
    };
    {
        let mut trace =
            vrcr_watchdog::diagnostics::resolve::ResolveTrace::new(&request, &playback, &ux);
        trace.reason = "resolve_timeout";
        trace.sent = "fallback_native";
        trace.retried = true;
    }
    let text = capture.text();
    assert!(text.contains(
        "resolve failed req=failed-one kind=resolve host=source.example status=unknown elapsed_ms="
    ));
    assert!(text.contains("bytes=0 retry=true sent=fallback_native reason=resolve_timeout"));
    assert_eq!(text.matches("session playback=failed-one").count(), 1);
    assert!(text.contains("ended=resolve_error"));
    private_text_is_absent(&text);
}

#[test]
fn diagnostics_failure_source_does_not_blame_the_server_for_a_network_timeout() {
    let playback = PlaybackLog::default();
    let (ux, _) = output();
    for (status, failure, source) in [
        (503, None, "server"),
        (0, Some("upstream_timeout"), "unknown"),
        (200, Some("client_disconnect"), "client"),
    ] {
        let mut request = sample(status);
        request.failure = failure.map(str::to_owned);
        playback.request(&request, "", now(1), &ux);
        assert_eq!(playback.context(TARGET).expect("context").source, source);
    }
}

#[test]
fn diagnostics_first_failure_is_visible_when_repeat_memory_is_full() {
    let playback = PlaybackLog::default();
    let (ux, capture) = output();
    for status in 400..465 {
        playback.request(&sample(status), "", now(u64::from(status)), &ux);
    }
    assert_eq!(capture.text().matches("request failed").count(), 65);
    assert!(capture.text().contains("status=464"));
}

#[test]
fn diagnostics_feedback_rejects_paths_and_unrecognised_failure_sources() {
    let context = vrcr_watchdog::diagnostics::FailureContext {
        playback_id: "private title".to_owned(),
        stream_id: "https://private.example/path".to_owned(),
        request_id: "C:/Users/private".to_owned(),
        source: "private/path".to_owned(),
        ..Default::default()
    };
    let payload = vrcr_watchdog::mesh::frames::playback_feedback_payload(
        vrcr_watchdog::mesh::frames::PlaybackFeedback {
            url: "https://media.example",
            kind: "load_failure",
            ms_since_open: 0,
            client_id: "existing",
            correlation_id: None,
            timestamp: now(1),
            delivered_height: None,
            detail: None,
            context: Some(&context),
        },
    );
    let value: serde_json::Value = serde_json::from_str(&payload).expect("JSON");
    for field in ["stream_id", "playback_id", "request_id", "failure_source"] {
        assert_eq!(value[field], "unknown");
    }
    assert!(!payload.contains("private"));
}
