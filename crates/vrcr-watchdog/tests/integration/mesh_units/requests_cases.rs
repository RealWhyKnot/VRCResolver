use super::*;

#[test]
fn resolve_log_line_defaults_the_level() {
    assert_eq!(
        resolve_log_line(&json!({"id":"r1","message":"trying tv"})),
        "[mesh][resolve_log] id=r1 level=info phase=trying config=tv"
    );
}

#[test]
fn resolve_request_defaults_to_the_resolve_action() {
    assert_eq!(ResolveRequest::default().action, action::RESOLVE);
    let parsed: ResolveRequest = serde_json::from_value(json!({"id":"a","url":"https://x"}))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(parsed.action, action::RESOLVE);
    let parsed: ResolveRequest = serde_json::from_value(json!({"action":"other","id":"a"}))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(parsed.action, "other");
}
