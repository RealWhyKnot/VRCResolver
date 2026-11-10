use std::fmt::Write as _;

use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::HeaderMap;
use http::header::HeaderValue;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use url::Url;
use vrcr_shared::b64;
use vrcr_shared::console::{Style, Ux};
use vrcr_shared::first_party::FirstPartyPolicy;
use vrcr_shared::log::Log;
use vrcr_watchdog::relay::activity::play_id;
use vrcr_watchdog::relay::headers::{
    copy_response_headers, forward_request_headers, should_drop_response_header,
};
use vrcr_watchdog::relay::hitch;
use vrcr_watchdog::relay::hitch::parse_lazy_segment;
use vrcr_watchdog::relay::manifest::{
    self, MAX_MANIFEST_BYTES, is_likely_manifest, localize, localize_stream,
};
use vrcr_watchdog::relay::security::{check_target_url, is_allowed_host_header};
use vrcr_watchdog::relay::server::query_value;
use vrcr_watchdog::relay::targets::{
    TargetKind, encode_target_param, is_play_path, local_prefix_for_path, resolve_relative_target,
};
use vrcr_watchdog::relay::{
    HitchDetector, RelayActivity, RelayBypass, RelayPortManager, TargetResolver, TimingSample,
    short_url,
};
use vrcr_watchdog::stats::RelayKind;

use crate::common::mesh::Capture;

fn policy() -> FirstPartyPolicy {
    FirstPartyPolicy::default()
}

fn capture_ux() -> (Ux, Capture) {
    let out = Capture::default();
    (
        Ux::new(Box::new(out.clone()), Style::plain(), String::new),
        out,
    )
}

fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn proxy(url: &str, name: &str) -> String {
    let normalized = Url::parse(url)
        .unwrap_or_else(|e| panic!("{e}"))
        .to_string();
    let ns = hex::encode(&Sha256::digest(normalized.as_bytes())[..8]);
    format!("proxy/{ns}/{name}?target={}", b64::encode(&normalized))
}

const SEG1: &str =
    "https://us1.vrcresolver.com/api/proxy/lazy-hls/wk_abc/seg_000001.ts?playback_id=tok";
const SEG2: &str = "https://eu1.vrcresolver.com/api/proxy/seg.ts?a=1&b=2";
const VARIANT: &str = "https://proxy.whyknot.dev/api/proxy/a/index.m3u8?x=1";
const KEY: &str = "https://eu1.vrcresolver.com/api/proxy/key.bin?clientId=c";
const MAP: &str = "https://us1.vrcresolver.com/api/proxy/init~mp4?q=%C3%A9";
const MEDIA: &str = "https://us1.vrcresolver.com/api/proxy/audio/en.m3u8?q=z";

fn golden_input(nl: &str) -> String {
    let lines = [
        "#EXTM3U".to_owned(),
        "#EXT-X-VERSION:6".to_owned(),
        format!("#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"{MEDIA}\""),
        "#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720".to_owned(),
        VARIANT.to_owned(),
        String::new(),
        format!("#EXT-X-KEY:METHOD=AES-128,URI=\"{KEY}\",IV=0x01"),
        format!("#EXT-X-MAP:URI=\"{MAP}\""),
        "#EXTINF:2.000,".to_owned(),
        SEG1.to_owned(),
        "#EXTINF:2.000,".to_owned(),
        format!("   {}  ", SEG2.replace('&', "&amp;")),
        "#EXTINF:2.000,".to_owned(),
        "seg_relative_3.ts".to_owned(),
        "#EXTINF:2.000,".to_owned(),
        "https://cdn.example.com/video/seg.ts?x=1".to_owned(),
        "data:text/plain;base64,AAAA".to_owned(),
        "\t".to_owned(),
        "#EXT-X-ENDLIST".to_owned(),
    ];
    format!("{}{nl}", lines.join(nl))
}

fn golden_expected() -> String {
    [
        "#EXTM3U".to_owned(),
        "#EXT-X-VERSION:6".to_owned(),
        format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"{}\"",
            proxy(MEDIA, "en.m3u8")
        ),
        "#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720".to_owned(),
        proxy(VARIANT, "index.m3u8"),
        String::new(),
        format!(
            "#EXT-X-KEY:METHOD=AES-128,URI=\"{}\",IV=0x01",
            proxy(KEY, "key.bin")
        ),
        format!("#EXT-X-MAP:URI=\"{}\"", proxy(MAP, "proxy.bin")),
        "#EXTINF:2.000,".to_owned(),
        proxy(SEG1, "seg_000001.ts"),
        "#EXTINF:2.000,".to_owned(),
        format!("   {}  ", proxy(SEG2, "seg.ts")),
        "#EXTINF:2.000,".to_owned(),
        "seg_relative_3.ts".to_owned(),
        "#EXTINF:2.000,".to_owned(),
        "https://cdn.example.com/video/seg.ts?x=1".to_owned(),
        "data:text/plain;base64,AAAA".to_owned(),
        "\t".to_owned(),
        "#EXT-X-ENDLIST".to_owned(),
    ]
    .join("\n")
}

fn chunked(
    input: &[u8],
    size: usize,
) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> + Unpin {
    let parts: Vec<std::io::Result<Bytes>> = input
        .chunks(size.max(1))
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    futures_util::stream::iter(parts)
}

fn stream_localize(input: &[u8], size: usize, max: u64) -> (String, manifest::LocalizeResult) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap_or_else(|e| panic!("{e}"));
    rt.block_on(async {
        let mut out: Vec<u8> = Vec::new();
        let r = localize_stream(&policy(), chunked(input, size), &mut out, max)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        (String::from_utf8(out).unwrap_or_else(|e| panic!("{e}")), r)
    })
}

fn sample(
    method: &str,
    local: &str,
    target: &str,
    status: u16,
    header: i64,
    total: i64,
) -> TimingSample {
    TimingSample {
        suppressed: false,
        recovery: false,
        retry_reason: None,
        request_id: "fixture-request".to_owned(),
        stream_id: String::new(),
        player_status: status,
        playlist_shape: None,
        method: method.to_owned(),
        local_path: local.to_owned(),
        target_url: target.to_owned(),
        status,
        header_ms: header,
        total_ms: total,
        bytes_out: 100,
        media: None,
        lazy_state: Some("HIT".to_owned()),
        lazy_wait_ms: -1,
        lazy_generator: None,
        failure: None,
    }
}

fn utc(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

mod activity_cases;

mod contracts_cases;

mod headers_cases;

mod hitches_cases;

mod manifests_cases;

mod live_cases;
mod live_review_cases;

mod ports_cases;

mod retry_cases;
mod targets_cases;
