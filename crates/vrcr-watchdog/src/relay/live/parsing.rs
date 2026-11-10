use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};
use vrcr_shared::logutil::{iso8601_utc, parse_iso8601_utc};

pub(super) fn tag(line: &str) -> (&str, &str) {
    line.strip_prefix('#')
        .map_or(("", ""), |line| line.split_once(':').unwrap_or((line, "")))
}

pub(crate) fn duration(value: &str) -> Option<Duration> {
    let (seconds, _) = value.split_once(',')?;
    if !seconds
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let duration = Duration::try_from_secs_f64(seconds.parse().ok()?).ok()?;
    (!duration.is_zero()).then_some(duration)
}

pub(super) fn range(value: &str) -> Option<(u64, Option<u64>)> {
    let (length, offset) = value
        .split_once('@')
        .map_or((value, None), |(length, offset)| (length, Some(offset)));
    if !length.bytes().all(|byte| byte.is_ascii_digit())
        || offset.is_some_and(|offset| !offset.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let length = length.parse().ok()?;
    if length == 0 {
        return None;
    }
    Some((length, offset.map(str::parse).transpose().ok()?))
}

pub(super) fn valid_header(name: &str, value: &str) -> bool {
    match name {
        "EXTM3U" | "EXT-X-INDEPENDENT-SEGMENTS" | "EXT-X-I-FRAMES-ONLY" => value.is_empty(),
        "EXT-X-VERSION" => {
            value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u64>().is_ok_and(|version| version > 0)
        }
        "EXT-X-ALLOW-CACHE" => matches!(value, "YES" | "NO"),
        "EXT-X-START" => attributes(value).is_some_and(|attrs| {
            attrs
                .get("TIME-OFFSET")
                .is_some_and(|offset| offset.parse::<f64>().is_ok_and(f64::is_finite))
                && attrs
                    .get("PRECISE")
                    .is_none_or(|value| matches!(*value, "YES" | "NO"))
        }),
        "EXT-X-SERVER-CONTROL" => attributes(value).is_some_and(|attrs| {
            !attrs.is_empty()
                && attrs.iter().all(|(name, value)| match *name {
                    "CAN-BLOCK-RELOAD" | "CAN-SKIP-DATERANGES" => matches!(*value, "YES" | "NO"),
                    "CAN-SKIP-UNTIL" | "HOLD-BACK" | "PART-HOLD-BACK" => value
                        .parse::<f64>()
                        .is_ok_and(|number| number.is_finite() && number >= 0.0),
                    _ => false,
                })
        }),
        _ => true,
    }
}

pub(super) fn attributes(mut text: &str) -> Option<HashMap<&str, &str>> {
    let mut result = HashMap::new();
    while !text.is_empty() {
        let (name, rest) = text.split_once('=')?;
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_uppercase() || matches!(byte, b'-' | b'_') || byte.is_ascii_digit()
            })
        {
            return None;
        }
        let (value, rest) = if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            (&quoted[..end], &quoted[end + 1..])
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        if value.is_empty() || result.insert(name, value).is_some() {
            return None;
        }
        if rest.is_empty() {
            break;
        }
        text = rest.strip_prefix(',')?;
        if text.is_empty() {
            return None;
        }
    }
    Some(result)
}

pub(super) fn date(text: &str) -> Option<Duration> {
    if !text.is_ascii() || text.len() < 20 {
        return None;
    }
    let year: u32 = text.get(..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if day == 0 || day > days {
        return None;
    }
    if !text.ends_with(['Z', 'z']) {
        let zone = text.get(text.len().checked_sub(6)?..)?;
        if !zone.starts_with(['+', '-']) || zone.get(3..4)? != ":" {
            return None;
        }
        if zone.get(1..3)?.parse::<u32>().ok()? > 23 || zone.get(4..)?.parse::<u32>().ok()? > 59 {
            return None;
        }
    }
    let parsed = parse_iso8601_utc(text)?.duration_since(UNIX_EPOCH).ok()?;
    let mut nanos = 0_u32;
    if let Some(fraction) = text.get(19..)?.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        nanos = fraction.get(..digits)?.parse::<u32>().ok()?;
        nanos *= 10_u32.pow(9 - u32::try_from(digits).ok()?);
    }
    Some(Duration::new(parsed.as_secs(), nanos))
}

pub(super) fn stamp(elapsed: Duration) -> Option<String> {
    let text = iso8601_utc(UNIX_EPOCH.checked_add(Duration::from_secs(elapsed.as_secs()))?);
    if text.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    Some(format!(
        "{}.{:09}Z",
        text.get(..19)?,
        elapsed.subsec_nanos()
    ))
}

pub(super) fn date_range_end(value: &str) -> Option<Option<Duration>> {
    let attrs = attributes(value)?;
    attrs.get("ID")?;
    let start = date(attrs.get("START-DATE")?)?;
    let end = match attrs.get("END-DATE") {
        Some(value) => Some(date(value)?),
        None => None,
    };
    let duration_end = if let Some(seconds) = attrs.get("DURATION") {
        let elapsed = Duration::try_from_secs_f64(seconds.parse().ok()?).ok()?;
        Some(start.checked_add(elapsed)?)
    } else {
        None
    };
    if end.is_some_and(|end| end < start)
        || end
            .zip(duration_end)
            .is_some_and(|(end, expected)| end != expected)
    {
        return None;
    }
    Some(end.or(duration_end))
}
