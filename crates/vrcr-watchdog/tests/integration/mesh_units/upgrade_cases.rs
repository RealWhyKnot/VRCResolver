use super::*;

#[test]
fn upgrade_request_offers_v3_and_no_extensions() {
    let req = upgrade_request("/mesh", "vrcresolver.com", "VRCResolver/1.2.3", KEY);
    assert!(req.starts_with("GET /mesh HTTP/1.1\r\n"));
    assert!(req.contains("\r\nHost: vrcresolver.com\r\n"));
    assert!(req.contains("\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"));
    assert!(req.contains(&format!("\r\nSec-WebSocket-Protocol: {SUBPROTOCOL_V3}\r\n")));
    assert!(req.contains("\r\nUser-Agent: VRCResolver/1.2.3\r\n"));
    assert!(req.contains("\r\nSec-WebSocket-Version: 13\r\n"));
    assert!(
        !req.to_ascii_lowercase()
            .contains("sec-websocket-extensions")
    );
    assert!(req.ends_with("\r\n\r\n"));
}

#[test]
fn upgrade_response_with_v3_is_accepted() {
    let head = ok_head(
        KEY,
        &format!("Sec-WebSocket-Protocol: {SUBPROTOCOL_V3}\r\n"),
    );
    assert_eq!(
        check_upgrade_response(&head, KEY),
        Ok(Some(SUBPROTOCOL_V3.to_owned()))
    );
}

#[test]
fn upgrade_response_without_a_subprotocol_is_legacy() {
    assert_eq!(check_upgrade_response(&ok_head(KEY, ""), KEY), Ok(None));
}

#[test]
fn upgrade_response_headers_are_case_insensitive() {
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: WebSocket\r\nconnection: keep-alive, Upgrade\r\nsec-websocket-accept: {}\r\n\r\n",
        derive_accept_key(KEY.as_bytes())
    );
    assert_eq!(check_upgrade_response(&head, KEY), Ok(None));
}

#[test]
fn upgrade_response_rejections() {
    let cases = [
        (
            "HTTP/1.1 503 Service Unavailable\r\n\r\n".to_owned(),
            "server returned HTTP 503 instead of 101",
        ),
        (
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                derive_accept_key(KEY.as_bytes())
            ),
            "upgrade response is missing Upgrade: websocket",
        ),
        (
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                derive_accept_key(KEY.as_bytes())
            ),
            "upgrade response is missing Connection: Upgrade",
        ),
        (
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: bogus\r\n\r\n"
                .to_owned(),
            "upgrade response carries the wrong Sec-WebSocket-Accept",
        ),
        (
            ok_head(KEY, "Sec-WebSocket-Extensions: permessage-deflate\r\n"),
            "server enabled a websocket extension that was not offered",
        ),
        (
            ok_head(KEY, "Sec-WebSocket-Protocol: vrcresolver-v9\r\n"),
            "server picked a subprotocol that was not offered: vrcresolver-v9",
        ),
    ];
    for (head, want) in cases {
        assert_eq!(
            check_upgrade_response(&head, KEY),
            Err(want.to_owned()),
            "{head}"
        );
    }
}
