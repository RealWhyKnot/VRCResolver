use std::borrow::Cow;

use crate::wire::{DEFAULT_MAX_HEIGHT, HIGH_QUALITY_MAX_HEIGHT, Player, ResolveRequest};

const HEIGHT_CAP_NEEDLE: &[u8] = b"height<=";

struct Cap {
    value_start: usize,
    value_end: usize,
    value: Option<i32>,
}

fn caps(format_arg: &str) -> impl Iterator<Item = Cap> + '_ {
    let bytes = format_arg.as_bytes();
    let mut from = 0;
    std::iter::from_fn(move || {
        let idx = find_ignore_case(bytes, HEIGHT_CAP_NEEDLE, from)?;
        let mut value_start = idx + HEIGHT_CAP_NEEDLE.len();
        if bytes.get(value_start) == Some(&b'?') {
            value_start += 1;
        }
        let value_end = value_start
            + bytes[value_start..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
        from = if value_end > value_start {
            value_end
        } else {
            idx + HEIGHT_CAP_NEEDLE.len()
        };
        let value = format_arg[value_start..value_end].parse::<i32>().ok();
        Some(Cap {
            value_start,
            value_end,
            value,
        })
    })
}

fn find_ignore_case(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() || needle.len() > hay.len() - from {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

pub fn height_cap(format_arg: Option<&str>) -> Option<i32> {
    caps(format_arg?).find_map(|c| c.value.filter(|v| *v > 0))
}

pub fn infer_player(format_arg: Option<&str>) -> Player {
    if height_cap(format_arg) == Some(720) {
        Player::Unity
    } else {
        Player::AvPro
    }
}

pub fn dash_f_value(args: &[String]) -> Option<&str> {
    args.windows(2)
        .find(|w| w[0] == "-f" || w[0] == "--format")
        .map(|w| w[1].as_str())
}

pub fn wrapper_request(
    id: String,
    url: &str,
    player: Player,
    format_arg: Option<&str>,
    skip_native_hint: bool,
) -> ResolveRequest {
    ResolveRequest {
        max_height: height_cap(format_arg),
        vrchat_format_arg: format_arg.map(str::to_owned),
        skip_native_hint: skip_native_hint.then_some(true),
        ..ResolveRequest::for_player(id, url.to_owned(), player)
    }
}

pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn apply_high_quality(request: &mut ResolveRequest, enabled: bool) -> bool {
    if !enabled || request.player != Some(Player::AvPro) {
        return false;
    }
    request.max_height = Some(HIGH_QUALITY_MAX_HEIGHT);
    request.vrchat_format_arg = None;
    request.prefer_highest = Some(true);
    true
}

pub fn apply_default_quality_cap(request: &mut ResolveRequest) -> bool {
    if request.player != Some(Player::AvPro) {
        return false;
    }
    let capped = match request.max_height {
        Some(h) if h > 0 => h.min(DEFAULT_MAX_HEIGHT),
        _ => DEFAULT_MAX_HEIGHT,
    };
    let mut changed = request.max_height != Some(capped);
    request.max_height = Some(capped);
    if let Some(arg) = request.vrchat_format_arg.as_deref()
        && let Cow::Owned(rewritten) = cap_height_in(arg, capped)
    {
        request.vrchat_format_arg = Some(rewritten);
        changed = true;
    }
    changed
}

pub fn cap_height_in(format_arg: &str, cap: i32) -> Cow<'_, str> {
    if cap <= 0 {
        return Cow::Borrowed(format_arg);
    }
    let mut out: Option<String> = None;
    let mut copied = 0;
    for c in caps(format_arg) {
        if c.value.is_some_and(|v| v > cap) {
            let s = out.get_or_insert_with(|| String::with_capacity(format_arg.len()));
            s.push_str(&format_arg[copied..c.value_start]);
            s.push_str(&cap.to_string());
            copied = c.value_end;
        }
    }
    match out {
        Some(mut s) => {
            s.push_str(&format_arg[copied..]);
            Cow::Owned(s)
        }
        None => Cow::Borrowed(format_arg),
    }
}
