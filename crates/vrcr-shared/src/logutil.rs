use url::Url;

pub fn sanitize_for_console(value: &str, max_len: usize) -> String {
    let max_len = max_len.max(1);
    let mut out = String::with_capacity(value.len().min(max_len) + 3);
    let mut chars = value.chars();
    for c in chars.by_ref().take(max_len) {
        let control = (c as u32) < 0x20 || matches!(c, '\u{7F}' | '\u{2028}' | '\u{2029}');
        out.push(if control { '?' } else { c });
    }
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

pub fn sanitize(value: &str) -> String {
    sanitize_for_console(value, 120)
}

pub fn redact_url(url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    match Url::parse(url) {
        Ok(u) => {
            let path = u.path();
            let path = match path.char_indices().nth(60) {
                Some((cut, _)) => format!("{}...", &path[..cut]),
                None => path.to_owned(),
            };
            format!("{}://{}{}", u.scheme(), u.host_str().unwrap_or_default(), path)
        }
        Err(_) => sanitize(url),
    }
}

pub fn bare_host(url: &str) -> String {
    match Url::parse(url) {
        Ok(u) => {
            let host = u.host_str().unwrap_or_default();
            match host.get(..4) {
                Some(p) if p.eq_ignore_ascii_case("www.") => host[4..].to_owned(),
                _ => host.to_owned(),
            }
        }
        Err(_) => "?".to_owned(),
    }
}

pub fn payload_preview(payload: &[u8], max_bytes: usize) -> String {
    if payload.is_empty() {
        return String::new();
    }
    let head = &payload[..payload.len().min(max_bytes)];
    sanitize_for_console(&String::from_utf8_lossy(head), max_bytes)
}

pub fn iso8601_utc(at: std::time::SystemTime) -> String {
    let since = at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let ticks = since.subsec_nanos() / 100;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ticks:07}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcStamp {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
    pub millis: u32,
}

impl UtcStamp {
    pub fn at(at: std::time::SystemTime) -> UtcStamp {
        let since = at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let secs = since.as_secs();
        let rem = secs % 86_400;
        let (year, month, day) = civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0));
        UtcStamp {
            year,
            month,
            day,
            hour: rem / 3600,
            minute: (rem % 3600) / 60,
            second: rem % 60,
            millis: since.subsec_millis(),
        }
    }

    pub fn log_prefix(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }

    pub fn file_stamp(&self) -> String {
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}-{:03}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub fn parse_iso8601_utc(text: &str) -> Option<std::time::SystemTime> {
    let t = text.trim();
    let b = t.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = t.get(r)?;
        s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse().ok())?
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &t[19..];
    let mut nanos: u64 = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        for (i, c) in frac.bytes().take(digits).enumerate() {
            if i < 9 {
                nanos = nanos * 10 + u64::from(c - b'0');
            }
        }
        for _ in digits..9 {
            nanos *= 10;
        }
        rest = &frac[digits..];
    }
    let offset_secs: i64 = match rest {
        "" | "Z" | "z" => 0,
        o if o.len() == 6 && matches!(&o[..1], "+" | "-") && &o[3..4] == ":" => {
            let h: i64 = o[1..3].parse().ok()?;
            let m: i64 = o[4..6].parse().ok()?;
            let v = h * 3600 + m * 60;
            if o.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    let days = days_from_civil(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    let secs = u64::try_from(secs).ok()?;
    std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(secs, u32::try_from(nanos).ok()?))
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
