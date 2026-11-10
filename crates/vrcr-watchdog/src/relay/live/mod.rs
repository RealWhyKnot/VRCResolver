use std::borrow::Cow;
use std::fmt::Write;
use std::time::Duration;

pub(crate) mod history;
mod parsing;
pub(crate) use parsing::duration;
use parsing::{attributes, date, date_range_end, range, stamp, tag, valid_header};

pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_SEGMENTS: usize = 100_000;
const MAX_LINES: usize = 1_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaInfo {
    pub bytes: u64,
    pub segments: usize,
    pub kept: usize,
    pub target_ms: u64,
    pub live: bool,
    pub needs_trim: bool,
    pub untrimmed: &'static str,
}

pub fn trim(body: &str) -> (Cow<'_, str>, Option<MediaInfo>) {
    let Some(playlist) = Playlist::parse(body) else {
        return (Cow::Borrowed(body), None);
    };
    let mut info = MediaInfo {
        bytes: u64::try_from(body.len()).unwrap_or(u64::MAX),
        segments: playlist.segments.len(),
        kept: playlist.segments.len(),
        target_ms: playlist.target.saturating_mul(1000),
        live: playlist.live,
        needs_trim: false,
        untrimmed: "bypass tag",
    };
    if !playlist.live || !playlist.safe {
        return (Cow::Borrowed(body), Some(info));
    }
    let mut elapsed = Duration::ZERO;
    let mut kept = 0;
    for segment in playlist.segments.iter().rev().take(120) {
        elapsed = elapsed.saturating_add(segment.duration);
        kept += 1;
        if kept >= 12 && elapsed >= Duration::from_secs(60) {
            break;
        }
    }
    let removed = info.segments - kept;
    info.needs_trim = removed > 0;
    if removed == 0 {
        return (Cow::Borrowed(body), Some(info));
    }
    match playlist.render(removed) {
        Some(mut output) => {
            info.kept = kept;
            info.untrimmed = "";
            if body.contains("\r\n") {
                output = output.replace('\n', "\r\n");
            }
            if body.starts_with('\u{feff}') {
                output.insert(0, '\u{feff}');
            }
            (Cow::Owned(output), Some(info))
        }
        None => (Cow::Borrowed(body), Some(info)),
    }
}

fn header(name: &str) -> bool {
    matches!(
        name,
        "EXTM3U"
            | "EXT-X-VERSION"
            | "EXT-X-TARGETDURATION"
            | "EXT-X-INDEPENDENT-SEGMENTS"
            | "EXT-X-START"
            | "EXT-X-SERVER-CONTROL"
            | "EXT-X-I-FRAMES-ONLY"
            | "EXT-X-ALLOW-CACHE"
    )
}

struct Segment<'a> {
    start: usize,
    uri: usize,
    duration: Duration,
    date: Option<Duration>,
    key: Option<&'a str>,
    map: Option<(&'a str, Option<&'a str>)>,
    range: Option<(u64, u64)>,
    discontinuities: u64,
}

struct Playlist<'a> {
    lines: Vec<&'a str>,
    segments: Vec<Segment<'a>>,
    sequence: u64,
    target: u64,
    live: bool,
    safe: bool,
}

impl<'a> Playlist<'a> {
    fn parse(body: &'a str) -> Option<Self> {
        if body.len() > MAX_INPUT_BYTES {
            return None;
        }
        let lines: Vec<_> = body
            .strip_prefix('\u{feff}')
            .unwrap_or(body)
            .lines()
            .take(MAX_LINES + 1)
            .map(str::trim)
            .collect();
        if lines.len() > MAX_LINES {
            return None;
        }
        if lines.first().copied()? != "#EXTM3U" {
            return None;
        }
        let mut playlist = Self {
            lines,
            segments: Vec::new(),
            sequence: 0,
            target: 0,
            live: true,
            safe: true,
        };
        let mut start = 0;
        let mut pending = None;
        let mut key = None;
        let mut map = None;
        let mut current_date: Option<Duration> = None;
        let mut pending_range = None;
        let mut previous_range: Option<(&str, u64)> = None;
        let mut discontinuities = 0_u64;
        let mut seen = std::collections::HashSet::new();
        for (index, line) in playlist.lines.iter().copied().enumerate() {
            let (name, value) = tag(line);
            match name {
                "EXT-X-STREAM-INF"
                | "EXT-X-MEDIA"
                | "EXT-X-I-FRAME-STREAM-INF"
                | "EXT-X-SESSION-DATA"
                | "EXT-X-SESSION-KEY" => return None,
                "EXT-X-ENDLIST" | "EXT-X-PLAYLIST-TYPE" => playlist.live = false,
                "EXT-X-PART"
                | "EXT-X-PART-INF"
                | "EXT-X-PRELOAD-HINT"
                | "EXT-X-RENDITION-REPORT"
                | "EXT-X-SKIP"
                | "EXT-X-DEFINE" => playlist.safe = false,
                "EXT-X-MEDIA-SEQUENCE"
                | "EXT-X-DISCONTINUITY-SEQUENCE"
                | "EXT-X-TARGETDURATION" => {
                    if !playlist.segments.is_empty() || !seen.insert(name) {
                        return None;
                    }
                    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                        return None;
                    }
                    let number = value.parse::<u64>().ok()?;
                    match name {
                        "EXT-X-MEDIA-SEQUENCE" => playlist.sequence = number,
                        "EXT-X-DISCONTINUITY-SEQUENCE" => discontinuities = number,
                        _ => {
                            if number == 0 {
                                return None;
                            }
                            playlist.target = number;
                        }
                    }
                }
                "EXTINF" => {
                    if pending.is_some() {
                        return None;
                    }
                    pending = Some(duration(value)?);
                }
                "EXT-X-KEY" => {
                    let attrs = attributes(value)?;
                    if attrs
                        .get("KEYFORMAT")
                        .is_some_and(|format| *format != "identity")
                    {
                        playlist.safe = false;
                    }
                    let method = *attrs.get("METHOD")?;
                    if method != "NONE" && !attrs.contains_key("URI") {
                        return None;
                    }
                    if !matches!(method, "NONE" | "AES-128" | "SAMPLE-AES" | "SAMPLE-AES-CTR") {
                        return None;
                    }
                    if method == "NONE" && attrs.len() != 1 {
                        return None;
                    }
                    if let Some(iv) = attrs.get("IV") {
                        let hex = iv.strip_prefix("0x").or_else(|| iv.strip_prefix("0X"))?;
                        if hex.is_empty()
                            || hex.len() > 32
                            || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                        {
                            return None;
                        }
                    }
                    key = Some(line);
                }
                "EXT-X-MAP" => {
                    let attrs = attributes(value)?;
                    attrs.get("URI")?;
                    if let Some(value) = attrs.get("BYTERANGE") {
                        let (_, offset) = range(value)?;
                        if offset.is_none() {
                            playlist.safe = false;
                        }
                    }
                    map = Some((line, key));
                }
                "EXT-X-PROGRAM-DATE-TIME" => current_date = Some(date(value)?),
                "EXT-X-BYTERANGE" => {
                    if pending_range.is_some() {
                        return None;
                    }
                    pending_range = Some(range(value)?);
                }
                "EXT-X-DISCONTINUITY" => discontinuities = discontinuities.checked_add(1)?,
                name if header(name) => {
                    if !playlist.segments.is_empty()
                        || !seen.insert(name)
                        || !valid_header(name, value)
                    {
                        return None;
                    }
                }
                "EXT-X-GAP" => {}
                "EXT-X-DATERANGE" => {
                    date_range_end(value)?;
                }
                "EXT-X-CUEPOINT" => {
                    attributes(value)?;
                }
                "" if !line.is_empty() && !line.starts_with('#') => {
                    if line.chars().any(char::is_whitespace) {
                        return None;
                    }
                    let duration = pending.take()?;
                    let resolved_range = if let Some((length, offset)) = pending_range.take() {
                        let offset = match offset {
                            Some(offset) => offset,
                            None => {
                                let (previous_uri, end) = previous_range?;
                                if previous_uri != line {
                                    return None;
                                }
                                end
                            }
                        };
                        previous_range = Some((line, offset.checked_add(length)?));
                        Some((length, offset))
                    } else {
                        previous_range = None;
                        None
                    };
                    if playlist.segments.len() == MAX_SEGMENTS {
                        return None;
                    }
                    playlist.segments.push(Segment {
                        start,
                        uri: index,
                        duration,
                        date: current_date,
                        key,
                        map,
                        range: resolved_range,
                        discontinuities,
                    });
                    if let Some(date) = current_date {
                        current_date = Some(date.checked_add(duration)?);
                    }
                    start = index + 1;
                }
                _ => {}
            }
        }
        if pending.is_some()
            || pending_range.is_some()
            || playlist.segments.is_empty()
            || playlist.target == 0
        {
            return None;
        }
        Some(playlist)
    }

    fn render(&self, removed: usize) -> Option<String> {
        let first = &self.segments[removed];
        let sequence = self.sequence.checked_add(u64::try_from(removed).ok()?)?;
        let mut output = String::new();
        for line in &self.lines[..first.start] {
            if header(tag(line).0) {
                writeln!(output, "{line}").ok()?;
            }
        }
        writeln!(output, "#EXT-X-MEDIA-SEQUENCE:{sequence}").ok()?;
        let boundary_discontinuities = self.lines[first.start..first.uri]
            .iter()
            .filter(|line| tag(line).0 == "EXT-X-DISCONTINUITY")
            .count();
        let discontinuities = first
            .discontinuities
            .checked_sub(u64::try_from(boundary_discontinuities).ok()?)?;
        writeln!(output, "#EXT-X-DISCONTINUITY-SEQUENCE:{discontinuities}").ok()?;
        for line in &self.lines[..first.start] {
            let (name, value) = tag(line);
            if name == "EXT-X-DATERANGE"
                && date_range_end(value)?
                    .zip(first.date)
                    .is_none_or(|(end, start)| end > start)
            {
                writeln!(output, "{line}").ok()?;
            }
        }
        if let Some((map, map_key)) = first.map {
            if let Some(key) = map_key {
                writeln!(output, "{key}").ok()?;
            }
            writeln!(output, "{map}").ok()?;
        }
        if let Some(key) = first.key
            && first.map.is_none_or(|(_, map_key)| map_key != Some(key))
        {
            writeln!(output, "{key}").ok()?;
        }
        if let Some(line) = self.lines[first.start..first.uri]
            .iter()
            .rev()
            .find(|line| tag(line).0 == "EXT-X-PROGRAM-DATE-TIME")
        {
            writeln!(output, "{line}").ok()?;
        } else if let Some(date) = first.date {
            writeln!(output, "#EXT-X-PROGRAM-DATE-TIME:{}", stamp(date)?).ok()?;
        }
        for (index, line) in self.lines.iter().enumerate().skip(first.start) {
            match tag(line).0 {
                "EXT-X-MEDIA-SEQUENCE" | "EXT-X-DISCONTINUITY-SEQUENCE" => {}
                "EXT-X-KEY" | "EXT-X-MAP" | "EXT-X-PROGRAM-DATE-TIME" if index < first.uri => {}
                "EXT-X-BYTERANGE" if index < first.uri => {
                    let (length, offset) = first.range?;
                    writeln!(output, "#EXT-X-BYTERANGE:{length}@{offset}").ok()?;
                }
                _ => {
                    writeln!(output, "{line}").ok()?;
                }
            }
        }
        Some(output)
    }
}
