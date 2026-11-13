use super::*;
use proptest::prelude::*;

#[test]
fn youtube_audio_content_is_decoded() {
    for (encoded, expected) in [
        (
            "ChEKBWFjb250EghvcmlnaW5hbAoKCgRsYW5nEgJqYQ",
            Some("original"),
        ),
        (
            "ChQKBWFjb250EgtkdWJiZWQtYXV0bwoNCgRsYW5nEgVlbi1VUw",
            Some("dubbed-auto"),
        ),
        ("CgoKBGxhbmcSAmph", None),
    ] {
        assert_eq!(audio_content(encoded).as_deref(), expected);
    }
}

#[test]
fn entries_and_fields_can_be_reordered() {
    let bytes = b"\x0a\x0a\x0a\x04lang\x12\x02ja\x0a\x11\x12\x08original\x0a\x05acont";
    assert_eq!(
        audio_content(&URL_SAFE_NO_PAD.encode(bytes)).as_deref(),
        Some("original")
    );
}

#[test]
fn unknown_length_delimited_fields_are_skipped() {
    let bytes = b"\x12\x02\xff\xff\x0a\x15\x1a\x02\xff\xff\x0a\x05acont\x12\x08original";
    assert_eq!(
        audio_content(&URL_SAFE_NO_PAD.encode(bytes)).as_deref(),
        Some("original")
    );
}

#[test]
fn scalar_fields_are_skipped_at_outer_and_entry_levels() {
    let extras: &[&[u8]] = &[
        b"\x18\x96\x01",
        b"\x1d\xff\xff\xff\xff",
        b"\x19\xff\xff\xff\xff\xff\xff\xff\xff",
        b"\x08\x01\x15\xff\xff\xff\xff",
    ];
    for extra in extras {
        let mut entry = extra.to_vec();
        entry.extend(b"\x0a\x05acont");
        entry.extend(*extra);
        entry.extend(b"\x12\x08original");
        entry.extend(*extra);
        let mut bytes = extra.to_vec();
        bytes.extend([0x0a, u8::try_from(entry.len()).expect("entry length")]);
        bytes.extend(entry);
        bytes.extend(*extra);
        assert_eq!(
            audio_content(&URL_SAFE_NO_PAD.encode(bytes)).as_deref(),
            Some("original"),
            "extra {extra:?}"
        );
    }
}

#[test]
fn truncated_scalar_fields_reject_the_message_at_both_levels() {
    let extras: &[&[u8]] = &[
        b"\x18\x80",
        b"\x1d\xff\xff\xff",
        b"\x19\xff\xff\xff\xff\xff\xff\xff",
    ];
    for extra in extras {
        let mut bytes = b"\x0a\x11\x0a\x05acont\x12\x08original".to_vec();
        bytes.extend(*extra);
        assert_eq!(audio_content(&URL_SAFE_NO_PAD.encode(&bytes)), None);
        bytes[1] += u8::try_from(extra.len()).expect("extra length");
        assert_eq!(audio_content(&URL_SAFE_NO_PAD.encode(bytes)), None);
    }
}

#[test]
fn multibyte_lengths_are_read() {
    let mut bytes = vec![0x12, 0x80, 0x01];
    bytes.extend([0xff; 128]);
    bytes.extend(b"\x0a\x11\x0a\x05acont\x12\x08original");
    assert_eq!(
        audio_content(&URL_SAFE_NO_PAD.encode(bytes)).as_deref(),
        Some("original")
    );
}

#[test]
fn invalid_base64_has_no_content() {
    for encoded in ["", "not base64!", "A", "____+///", "日本語", "Cg="] {
        assert_eq!(audio_content(encoded), None, "{encoded}");
    }
}

#[test]
fn malformed_protobuf_has_no_content() {
    let cases: &[&[u8]] = &[
        b"\x0a",
        b"\x0a\x11\x0a\x05acont\x12\x08origina",
        b"\x0a\x10\x0a\x05acont\x12\x08origina",
        b"\x0a\x11\x0a\x05acont\x12\x08original\x0a",
        b"\x0a\x08\x0a\x01\xff\x12\x03bad",
        b"\x00\x00",
        b"\x08\x00",
        b"\x0a\xff\xff\xff\xff\xff\xff\xff\xff\xff\x02",
        b"\x0a\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01",
        b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x00",
    ];
    for bytes in cases {
        assert_eq!(audio_content(&URL_SAFE_NO_PAD.encode(bytes)), None);
    }
}

proptest! {
    #[test]
    fn arbitrary_xtags_never_panic(encoded in any::<String>()) {
        let _ = audio_content(&encoded);
    }

    #[test]
    fn arbitrary_protobuf_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let _ = audio_content(&URL_SAFE_NO_PAD.encode(bytes));
    }
}
