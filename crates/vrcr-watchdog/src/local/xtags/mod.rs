use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub(super) fn audio_content(encoded: &str) -> Option<String> {
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let mut remaining = decoded.as_slice();
    let mut content = None;
    while !remaining.is_empty() {
        let (1, Some(mut entry)) = field(&mut remaining)? else {
            continue;
        };
        let mut key = None;
        let mut value = None;
        while !entry.is_empty() {
            let (number, Some(text)) = field(&mut entry)? else {
                continue;
            };
            match number {
                1 => key = Some(std::str::from_utf8(text).ok()?),
                2 => value = Some(std::str::from_utf8(text).ok()?),
                _ => {}
            }
        }
        if key == Some("acont") {
            content = value;
        }
    }
    content.map(str::to_owned)
}

fn field<'a>(remaining: &mut &'a [u8]) -> Option<(u64, Option<&'a [u8]>)> {
    let tag = varint(remaining)?;
    let number = tag >> 3;
    if !(1..=0x1fff_ffff).contains(&number) {
        return None;
    }
    let wire_type = tag & 7;
    let length = match wire_type {
        0 => {
            varint(remaining)?;
            return Some((number, None));
        }
        1 => 8,
        2 => usize::try_from(varint(remaining)?).ok()?,
        5 => 4,
        _ => return None,
    };
    let (value, rest) = remaining.split_at_checked(length)?;
    *remaining = rest;
    Some((number, (wire_type == 2).then_some(value)))
}

fn varint(remaining: &mut &[u8]) -> Option<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = remaining.split_first()?;
        *remaining = rest;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests;
