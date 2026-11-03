use super::*;

#[tokio::test]
async fn cached_welcome_alone_keeps_the_reconnect_backoff() {
    let h = Harness::v3(true);
    h.client
        .inner
        .reconnect_attempt
        .store(4, std::sync::atomic::Ordering::SeqCst);

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"node":"node1"}"#)
        .await;

    assert_eq!(h.client.reconnect_attempt(), 4);
    assert_eq!(h.client.server_protocol_version(), 3);
}

#[tokio::test]
async fn full_welcome_alone_keeps_the_reconnect_backoff() {
    let h = Harness::v3(false);
    h.client
        .inner
        .reconnect_attempt
        .store(4, std::sync::atomic::Ordering::SeqCst);

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1"}"#)
        .await;

    assert_eq!(h.client.reconnect_attempt(), 4);
}

#[tokio::test]
async fn cached_welcome_without_a_local_entry_keeps_the_backoff() {
    let h = Harness::v3(false);
    h.client
        .inner
        .reconnect_attempt
        .store(4, std::sync::atomic::Ordering::SeqCst);

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"node":"node1"}"#)
        .await;

    assert_eq!(h.client.reconnect_attempt(), 4);
    assert!(
        h.out
            .text()
            .contains("welcome_cached but local entry missing -- invalidating + reconnecting")
    );
}

#[tokio::test]
async fn welcome_records_the_server_and_applies_its_hosts() {
    let h = Harness::new();

    h.json(
        r#"{"action":"welcome","protocol_version":9,"node":"node1","server_version":"s1","yt_dlp_version":"y1","warp_active":true,"features":["welcome_hosts"],"first_party_hosts":["example.org"],"playback_proxy_paths":["/p"]}"#,
    )
    .await;

    assert_eq!(h.client.server_protocol_version(), 3);
    assert_eq!(h.client.server_node().as_deref(), Some("node1"));
    assert_eq!(h.client.server_version().as_deref(), Some("s1"));
    assert_eq!(h.client.yt_dlp_version().as_deref(), Some("y1"));
    assert_eq!(h.client.warp_active(), Some(true));
    let policy = h.client.first_party();
    let policy = policy.read().unwrap_or_else(|e| e.into_inner());
    assert!(policy.host_families().iter().any(|f| f == "example.org"));
    drop(policy);
    assert!(h.out.text().contains(
        "welcome node=node1 v=9 (negotiated=3) server=s1 yt-dlp=y1 warp_active=true features=[welcome_hosts]"
    ));
}

#[tokio::test]
async fn welcome_without_features_warns_and_keeps_the_default_hosts() {
    let h = Harness::new();
    let before = h
        .client
        .first_party()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .host_families()
        .to_vec();

    h.json(r#"{"action":"welcome","protocol_version":1,"node":"fake","first_party_hosts":["evil.example"]}"#)
        .await;

    assert!(
        h.out
            .text()
            .contains("welcome missing required field: features")
    );
    let after = h
        .client
        .first_party()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .host_families()
        .to_vec();
    assert_eq!(before, after);
}

#[tokio::test]
async fn v3_welcome_with_a_hash_is_cached_for_the_next_connect() {
    let h = Harness::v3(false);

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h9"}"#)
        .await;

    let entry = h
        .client
        .inner
        .welcome_cache
        .get(PROXY_HOST)
        .unwrap_or_else(|| panic!("not cached"));
    assert_eq!(entry.welcome_hash.as_deref(), Some("h9"));
}

#[tokio::test]
async fn welcome_on_a_non_v3_link_is_not_cached() {
    let h = Harness::new();
    super::super::lock(&h.client.inner.server).current_host = PROXY_HOST.to_owned();

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h9"}"#)
        .await;

    assert!(h.client.inner.welcome_cache.get(PROXY_HOST).is_none());
}

#[tokio::test]
async fn cached_welcome_hydrates_the_server_from_the_local_entry() {
    let h = Harness::v3(true);

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"warp_active":true,"negotiated_format":"msgpack"}"#)
        .await;

    assert_eq!(h.client.server_node().as_deref(), Some("node1"));
    assert_eq!(h.client.server_version().as_deref(), Some("s1"));
    assert_eq!(h.client.warp_active(), Some(true));
    assert_eq!(h.client.negotiated_format(), "msgpack");
    assert!(h.client.inner.is_msgpack());
    let policy = h.client.first_party();
    assert!(
        policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .host_families()
            .iter()
            .any(|f| f == "example.org")
    );
    assert!(
        h.log()
            .contains("[mesh][v3] welcome_cached hit node=node1 v=3 negotiated_format=msgpack")
    );
}

#[tokio::test]
async fn cached_welcome_on_a_non_v3_link_is_a_protocol_error() {
    let h = Harness::new();

    h.json(r#"{"action":"welcome_cached","protocol_version":3}"#)
        .await;

    assert!(
        h.out
            .text()
            .contains("welcome_cached received on non-v3 connection")
    );
    assert_eq!(h.client.server_protocol_version(), 0);
}

#[tokio::test]
async fn malformed_welcome_does_not_log_invalid_values() {
    let h = Harness::new();
    h.json(r#"{"action":"welcome","protocol_version":"private-version"}"#)
        .await;
    let output = format!("{}{}", h.out.text(), h.log());
    assert!(output.contains("welcome parse failed"));
    assert!(!output.contains("private-version"), "{output}");
}
