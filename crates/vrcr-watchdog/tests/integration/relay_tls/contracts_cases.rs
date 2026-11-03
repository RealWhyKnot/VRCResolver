use super::*;

#[test]
fn thumbprint_is_uppercase_sha1() {
    assert_eq!(
        tls::thumbprint(b"abc"),
        "A9993E364706816ABA3E25717850C26C9CD0D89D"
    );
}

#[test]
fn disable_switch_accepts_one_or_true() {
    for (v, expected) in [
        (Some("1"), true),
        (Some("true"), true),
        (Some("TRUE"), true),
        (Some("yes"), false),
        (Some("0"), false),
        (Some(""), false),
        (None, false),
    ] {
        assert_eq!(tls::disabled_value(v), expected, "{v:?}");
    }
}
