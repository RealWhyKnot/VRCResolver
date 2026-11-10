use std::borrow::Cow;
use std::future::Future;
use std::io;
use std::sync::LazyLock;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use regex_lite::{Captures, Regex};
use sha2::{Digest, Sha256};
use url::Url;
use vrcr_shared::b64;
use vrcr_shared::first_party::{FirstPartyPolicy, allowed_extension_from_path};

use super::live;

pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_CONTENT_TYPE: &str = "application/vnd.apple.mpegurl";

const FLUSH_THRESHOLD: usize = 8 * 1024;
const BOM: &[u8] = b"\xEF\xBB\xBF";

static ABSOLUTE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)https?://[^\s"'<>]+"#).unwrap_or_else(|e| panic!("{e}")));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalizeResult {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub changed: bool,
    pub exceeded: bool,
    pub media: Option<live::MediaInfo>,
    pub kind: PlaylistKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaylistKind {
    #[default]
    Unknown,
    Master,
    Media,
}

impl LocalizeResult {
    pub fn shape(&self) -> String {
        let kind = match self.kind {
            PlaylistKind::Unknown => "unknown",
            PlaylistKind::Master => "master",
            PlaylistKind::Media => "media",
        };
        let media = self.media.unwrap_or_default();
        let mode = if self.kind != PlaylistKind::Media {
            "unknown"
        } else if media.live {
            "live"
        } else {
            "vod"
        };
        format!(
            "kind={kind} mode={mode} segments={} target_ms={} bytes_in={} bytes_out={} trimmed={} kept={}",
            media.segments,
            media.target_ms,
            self.bytes_in,
            self.bytes_out,
            media.kept < media.segments,
            media.kept
        )
    }
}

pub trait ManifestSink {
    fn write(&mut self, chunk: Bytes) -> impl Future<Output = io::Result<()>> + Send;
}

impl ManifestSink for Vec<u8> {
    async fn write(&mut self, chunk: Bytes) -> io::Result<()> {
        self.extend_from_slice(&chunk);
        Ok(())
    }
}

pub fn is_likely_manifest(local_path: &str, target_url: &str, content_type: Option<&str>) -> bool {
    if has_manifest_extension(local_path) || looks_like_manifest_path(local_path) {
        return true;
    }
    if let Ok(u) = Url::parse(target_url)
        && (has_manifest_extension(u.path()) || looks_like_manifest_path(u.path()))
    {
        return true;
    }
    let media = content_type
        .and_then(|c| c.split(';').next())
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    media.contains("mpegurl")
        || media.contains("dash+xml")
        || media == "application/xml"
        || media == "text/xml"
}

pub fn localize(policy: &FirstPartyPolicy, content: &str) -> String {
    let mut out = String::with_capacity(content.len() + 256);
    for (i, line) in read_lines(content).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&localize_line(policy, line));
    }
    out
}

pub async fn localize_stream<S, W>(
    policy: &FirstPartyPolicy,
    input: S,
    out: &mut W,
    max_bytes: u64,
) -> io::Result<LocalizeResult>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
    W: ManifestSink,
{
    localize_stream_with_ending(policy, input, out, max_bytes, b"\n").await
}

async fn localize_stream_with_ending<S, W>(
    policy: &FirstPartyPolicy,
    mut input: S,
    out: &mut W,
    max_bytes: u64,
    ending: &[u8],
) -> io::Result<LocalizeResult>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
    W: ManifestSink,
{
    let mut result = LocalizeResult::default();
    let mut splitter = LineSplitter::default();
    let mut pending = BytesMut::with_capacity(FLUSH_THRESHOLD * 2);
    let mut first = true;
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut done = false;
    let mut media = live::MediaInfo {
        live: true,
        ..Default::default()
    };
    let mut hls = false;
    let mut master = false;
    let mut tail_duration = Duration::ZERO;
    while !done {
        match input.next().await {
            Some(chunk) => {
                let chunk = chunk?;
                media.bytes = media
                    .bytes
                    .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
                splitter.push(&chunk, &mut lines);
            }
            None => {
                splitter.finish(&mut lines);
                done = true;
            }
        }
        for raw in lines.drain(..) {
            result.bytes_in += u64::try_from(raw.len()).expect("length fits the byte counter");
            let text = String::from_utf8_lossy(&raw);
            match text.trim().split(':').next().unwrap_or_default() {
                "#EXTM3U" => hls = true,
                "#EXT-X-STREAM-INF" | "#EXT-X-MEDIA" | "#EXT-X-I-FRAME-STREAM-INF" => master = true,
                "#EXTINF" => {
                    media.segments += 1;
                    if media.segments > 1
                        && let Some(duration) =
                            text.strip_prefix("#EXTINF:").and_then(live::duration)
                    {
                        tail_duration = tail_duration.saturating_add(duration);
                    }
                }
                "#EXT-X-ENDLIST" | "#EXT-X-PLAYLIST-TYPE" => media.live = false,
                "#EXT-X-TARGETDURATION" => {
                    media.target_ms = text
                        .split_once(':')
                        .and_then(|(_, value)| value.parse::<u64>().ok())
                        .and_then(|value| value.checked_mul(1000))
                        .unwrap_or(0);
                }
                _ => {}
            }
            let localized = localize_line(policy, &text);
            if matches!(localized, Cow::Owned(_)) {
                result.changed = true;
            }
            if !first {
                pending.extend_from_slice(ending);
                result.bytes_out += u64::try_from(ending.len()).unwrap_or(0);
            }
            first = false;
            pending.extend_from_slice(localized.as_bytes());
            result.bytes_out +=
                u64::try_from(localized.len()).expect("length fits the byte counter");
            if pending.len() >= FLUSH_THRESHOLD {
                out.write(pending.split().freeze()).await?;
            }
            if result.bytes_out > max_bytes {
                result.exceeded = true;
                done = true;
                break;
            }
        }
    }
    if !pending.is_empty() {
        out.write(pending.freeze()).await?;
    }
    result.bytes_in = media.bytes;
    result.kind = if !hls {
        PlaylistKind::Unknown
    } else if master {
        PlaylistKind::Master
    } else {
        PlaylistKind::Media
    };
    if hls && !master {
        media.kept = media.segments;
        media.needs_trim = media.segments > 120
            || (media.segments > 12 && tail_duration >= Duration::from_secs(60));
        result.media = Some(media);
    }
    Ok(result)
}

pub async fn localize_relay_stream<S, W>(
    policy: &FirstPartyPolicy,
    input: S,
    out: &mut W,
    max_bytes: u64,
) -> io::Result<LocalizeResult>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
    W: ManifestSink,
{
    localize_relay_stream_with_budget(
        policy,
        input,
        out,
        max_bytes,
        Duration::from_secs(1),
        Buffering::Unknown,
    )
    .await
}

pub(crate) enum Buffering {
    Unknown,
    Live,
    Small,
}

pub(crate) async fn localize_relay_stream_with_budget<S, W>(
    policy: &FirstPartyPolicy,
    mut input: S,
    out: &mut W,
    max_bytes: u64,
    budget: Duration,
    buffering: Buffering,
) -> io::Result<LocalizeResult>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
    W: ManifestSink,
{
    let mut buffered = BytesMut::new();
    let mut splitter = LineSplitter::default();
    let mut lines = Vec::new();
    let mut first_line = true;
    let started = tokio::time::Instant::now();
    let mut segments = 0;
    let mut live_header = matches!(buffering, Buffering::Live);
    let stream_directly = matches!(buffering, Buffering::Small);
    loop {
        let deadline = started
            + if segments > 120 {
                budget
            } else {
                budget.min(Duration::from_millis(50))
            };
        let next = if live_header {
            Some(input.next().await)
        } else if tokio::time::Instant::now() >= deadline {
            None
        } else {
            tokio::time::timeout_at(deadline, input.next()).await.ok()
        };
        let chunk = match next {
            Some(Some(Ok(chunk))) => chunk,
            Some(None) => break,
            Some(Some(Err(error))) => {
                let prefix = futures_util::stream::iter([Ok(buffered.freeze()), Err(error)]);
                return localize_stream(policy, prefix, out, max_bytes).await;
            }
            None => {
                let prefix = futures_util::stream::iter([Ok(buffered.freeze())]);
                return stream_untrimmed(policy, prefix.chain(input), out, max_bytes, "budget")
                    .await;
            }
        };
        if buffered.len().saturating_add(chunk.len()) > live::MAX_INPUT_BYTES {
            let prefix = futures_util::stream::iter([Ok(buffered.freeze()), Ok(chunk)]);
            return stream_untrimmed(policy, prefix.chain(input), out, max_bytes, "size limit")
                .await;
        }
        buffered.extend_from_slice(&chunk);
        splitter.push(&chunk, &mut lines);
        let bypass = lines.drain(..).any(|line| {
            let line = String::from_utf8_lossy(&line);
            if line.starts_with("#EXTINF:") {
                segments += 1;
            }
            if segments == 0
                && line
                    .strip_prefix("#EXT-X-MEDIA-SEQUENCE:")
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .is_some_and(|sequence| sequence > 0)
            {
                live_header = true;
            }
            if first_line {
                first_line = false;
                if line.trim() != "#EXTM3U" {
                    return true;
                }
            }
            matches!(
                line.trim().split(':').next().unwrap_or_default(),
                "#EXT-X-ENDLIST"
                    | "#EXT-X-PLAYLIST-TYPE"
                    | "#EXT-X-STREAM-INF"
                    | "#EXT-X-MEDIA"
                    | "#EXT-X-I-FRAME-STREAM-INF"
                    | "#EXT-X-PART"
                    | "#EXT-X-PART-INF"
                    | "#EXT-X-PRELOAD-HINT"
                    | "#EXT-X-RENDITION-REPORT"
                    | "#EXT-X-SKIP"
                    | "#EXT-X-DEFINE"
            )
        });
        if bypass
            || (stream_directly && !live_header)
            || u64::try_from(buffered.len()).unwrap_or(u64::MAX) > max_bytes
        {
            let prefix = futures_util::stream::iter([Ok(buffered.freeze())]);
            return stream_untrimmed(
                policy,
                prefix.chain(input),
                out,
                max_bytes,
                if bypass {
                    "bypass tag"
                } else if stream_directly && !live_header {
                    "budget"
                } else {
                    "size limit"
                },
            )
            .await;
        }
    }
    let (body, media) = match std::str::from_utf8(&buffered) {
        Ok(body) => {
            let (trimmed, media) = live::trim(body);
            (Bytes::copy_from_slice(trimmed.as_bytes()), media)
        }
        Err(_) => (buffered.clone().freeze(), None),
    };
    let trimmed = media.is_some_and(|info| info.kept < info.segments);
    let ending: &[u8] = if trimmed && buffered.windows(2).any(|pair| pair == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let mut result = localize_stream_with_ending(
        policy,
        futures_util::stream::iter([Ok(body)]),
        out,
        max_bytes,
        ending,
    )
    .await?;
    result.bytes_in = u64::try_from(buffered.len()).unwrap_or(u64::MAX);
    result.changed |= trimmed;
    if media.is_some() {
        result.media = media;
    }
    Ok(result)
}

async fn stream_untrimmed<S, W>(
    policy: &FirstPartyPolicy,
    input: S,
    out: &mut W,
    max_bytes: u64,
    reason: &'static str,
) -> io::Result<LocalizeResult>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
    W: ManifestSink,
{
    let mut result = localize_stream(policy, input, out, max_bytes).await?;
    if let Some(media) = result.media.as_mut() {
        media.untrimmed = reason;
    }
    Ok(result)
}

pub fn localize_line<'a>(policy: &FirstPartyPolicy, line: &'a str) -> Cow<'a, str> {
    let mut localized: Cow<'a, str> = if line.contains("://") {
        rewrite_absolute_urls(policy, line)
    } else {
        Cow::Borrowed(line)
    };
    if !localized.starts_with('#') && !localized.trim().is_empty() {
        let lead = localized.len() - localized.trim_start().len();
        let trimmed = localized.trim();
        if !starts_with_ignore_case(trimmed, "data:")
            && let Some(replacement) = build_local_target(policy, trimmed)
        {
            let tail = lead + trimmed.len();
            let spliced = format!("{}{replacement}{}", &localized[..lead], &localized[tail..]);
            localized = Cow::Owned(spliced);
        }
    }
    localized
}

fn rewrite_absolute_urls<'a>(policy: &FirstPartyPolicy, line: &'a str) -> Cow<'a, str> {
    let mut hit = false;
    let replaced = ABSOLUTE_URL.replace_all(line, |caps: &Captures<'_>| {
        let found = &caps[0];
        match build_local_target(policy, &html_decode(found.trim())) {
            Some(r) => {
                hit = true;
                r
            }
            None => found.to_owned(),
        }
    });
    if hit {
        Cow::Owned(replaced.into_owned())
    } else {
        Cow::Borrowed(line)
    }
}

pub fn build_local_target(policy: &FirstPartyPolicy, original: &str) -> Option<String> {
    let url = Url::parse(original).ok()?;
    if !policy.is_playback_proxy(&url) {
        return None;
    }
    let ext = match allowed_extension_from_path(url.path()) {
        "" => "bin",
        e => e,
    };
    let file = url.path().rsplit('/').next().unwrap_or_default();
    let name = if allowed_extension_from_path(url.path()).is_empty()
        && !file.to_ascii_lowercase().ends_with(".bin")
    {
        format!("proxy.{ext}")
    } else {
        file.to_owned()
    };
    let normalized = url.to_string();
    Some(format!(
        "proxy/{}/{name}?target={}",
        target_namespace(&normalized),
        b64::encode(&normalized)
    ))
}

pub fn target_namespace(normalized: &str) -> String {
    let hash = Sha256::digest(normalized.as_bytes());
    hex::encode(&hash[..8])
}

pub fn html_decode(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        match tail
            .find(';')
            .filter(|semi| *semi <= 12)
            .and_then(|semi| decode_entity(&tail[1..semi]).map(|c| (c, semi)))
        {
            Some((c, semi)) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{A0}'),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }
    }
}

fn has_manifest_extension(path: &str) -> bool {
    matches!(allowed_extension_from_path(path), "m3u8" | "mpd")
}

fn looks_like_manifest_path(path: &str) -> bool {
    let file = path.rsplit(['/', '\\']).next().unwrap_or_default();
    if file.eq_ignore_ascii_case("manifest") {
        return true;
    }
    starts_with_ignore_case(file, "manifest.") && has_manifest_extension(file)
}

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn read_lines(content: &str) -> impl Iterator<Item = &str> {
    let mut rest = content;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        match rest.find(['\r', '\n']) {
            Some(i) => {
                let line = &rest[..i];
                let skip = if rest[i..].starts_with("\r\n") { 2 } else { 1 };
                rest = &rest[i + skip..];
                Some(line)
            }
            None => {
                let line = rest;
                rest = "";
                Some(line)
            }
        }
    })
}

#[derive(Default)]
struct LineSplitter {
    buf: Vec<u8>,
    started: bool,
    skip_lf: bool,
}

impl LineSplitter {
    fn push(&mut self, chunk: &[u8], lines: &mut Vec<Vec<u8>>) {
        let mut chunk = chunk;
        if !self.started {
            let have = self.buf.len() + chunk.len();
            if have < BOM.len() && BOM.starts_with(&[self.buf.as_slice(), chunk].concat()) {
                self.buf.extend_from_slice(chunk);
                return;
            }
            self.started = true;
            let mut head = std::mem::take(&mut self.buf);
            head.extend_from_slice(chunk);
            let body = head.strip_prefix(BOM).unwrap_or(&head).to_vec();
            self.scan(&body, lines);
            return;
        }
        if self.skip_lf {
            self.skip_lf = false;
            if let Some(rest) = chunk.strip_prefix(b"\n") {
                chunk = rest;
            }
        }
        self.scan(chunk, lines);
    }

    fn scan(&mut self, mut chunk: &[u8], lines: &mut Vec<Vec<u8>>) {
        while let Some(i) = chunk.iter().position(|b| *b == b'\r' || *b == b'\n') {
            let mut line = std::mem::take(&mut self.buf);
            line.extend_from_slice(&chunk[..i]);
            lines.push(line);
            let was_cr = chunk[i] == b'\r';
            chunk = &chunk[i + 1..];
            if was_cr {
                match chunk.first() {
                    Some(b'\n') => chunk = &chunk[1..],
                    Some(_) => {}
                    None => self.skip_lf = true,
                }
            }
        }
        self.buf.extend_from_slice(chunk);
    }

    fn finish(&mut self, lines: &mut Vec<Vec<u8>>) {
        if !self.started {
            self.started = true;
            let head = std::mem::take(&mut self.buf);
            if head.as_slice() != BOM && !head.is_empty() {
                self.scan(&head, lines);
            }
        }
        if !self.buf.is_empty() {
            lines.push(std::mem::take(&mut self.buf));
        }
    }
}
