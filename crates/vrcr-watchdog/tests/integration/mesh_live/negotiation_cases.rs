use super::*;

#[tokio::test]
async fn v3_link_sends_client_hello_and_reuses_the_cached_welcome_on_reconnect() {
    let mut opts = Options::new(policy(|_| Mode::Accept));
    opts.v3 = true;
    opts.welcome = v3_welcome(false);
    let server = Server::start(opts).await;
    let t = client_for(&server, |c| c.next_backoff_ms = fixed_backoff(50));
    t.client.start();
    wait_until(
        || t.client.server_protocol_version() == 3,
        5_000,
        "v3 welcome",
    )
    .await;

    let first = resolve(&t.client, "r1", Duration::from_secs(10)).await;
    t.client.drop_connection();
    wait_until(|| server.connections() >= 2, 5_000, "second connection").await;
    let second = resolve(&t.client, "r2", Duration::from_secs(10)).await;

    assert_eq!(first.action, action::RESOLVED);
    assert_eq!(second.action, action::RESOLVED);
    let hellos: Vec<Value> = server.record(|r| {
        r.texts
            .iter()
            .filter(|(_, v)| v["action"] == action::CLIENT_HELLO)
            .map(|(_, v)| v.clone())
            .collect()
    });
    assert_eq!(hellos.len(), 2);
    assert!(hellos[0].get("welcome_hash").is_none());
    assert_eq!(hellos[1]["welcome_hash"], "hash1");
    assert_eq!(
        hellos[0]["accept_formats"],
        serde_json::json!(["msgpack", "json"])
    );
    assert_eq!(hellos[0]["client_id"].as_str().map(str::len), Some(32));
    assert_eq!(hellos[0]["client_id"], hellos[1]["client_id"]);
    assert!(
        t.log()
            .contains("[mesh][v3] welcome_cached hit node=node1 v=3")
    );
    assert_eq!(t.client.server_version().as_deref(), Some("s1"));
    t.client.stop().await;
}

#[tokio::test]
async fn v3_client_id_survives_a_restart() {
    let mut opts = Options::new(policy(|_| Mode::Accept));
    opts.v3 = true;
    opts.welcome = v3_welcome(false);
    let server = Server::start(opts).await;
    let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().to_path_buf();
    let t = client_in(dir, &server, |_| {});
    let id = t.client.client_id().to_owned();
    let again = vrcr_watchdog::mesh::identity::load_or_create(
        &path.join(vrcr_watchdog::mesh::identity::FILE_NAME),
    );
    assert_eq!(id, again);
}

#[tokio::test]
async fn msgpack_link_decodes_binary_answers() {
    let mut opts = Options::new(policy(|_| Mode::Accept)).on_resolve(handler(
        |c: Conn, id: String| async move {
            let mut out = Vec::new();
            let frame = rmpv::Value::Array(vec![
                rmpv::Value::from(action::RESOLVED),
                rmpv::Value::from(id.as_str()),
                rmpv::Value::from(MEDIA),
            ]);
            rmpv::encode::write_value(&mut out, &frame).unwrap_or_else(|e| panic!("{e}"));
            c.binary(out);
        },
    ));
    opts.v3 = true;
    opts.welcome = v3_welcome(true);
    let server = Server::start(opts).await;
    let t = client_for(&server, |_| {});
    t.client.start();
    wait_until(
        || t.client.negotiated_format() == "msgpack",
        5_000,
        "msgpack welcome",
    )
    .await;

    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    let frame: Value = serde_json::from_slice(&result.frame).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(frame["url"], MEDIA);
    assert_eq!(frame["id"], "r1");
    t.client.stop().await;
}

#[tokio::test]
async fn binary_frame_on_a_json_link_drops_the_connection() {
    let server = Server::start(Options::new(policy(|_| Mode::Accept)).on_resolve(handler(
        |c: Conn, id: String| async move {
            if c.index == 1 {
                c.binary(vec![0x91, 0xA4, b'p', b'o', b'n', b'g']);
            } else {
                c.resolved(&id);
            }
        },
    )))
    .await;
    let t = client_for(&server, |c| c.next_backoff_ms = fixed_backoff(50));
    t.client.start();

    let result = resolve(&t.client, "r1", Duration::from_secs(10)).await;

    assert_eq!(result.action, action::RESOLVED);
    assert_eq!(server.connections(), 2);
    assert!(
        t.out
            .text()
            .contains("unexpected Binary frame on json-negotiated connection")
    );
    t.client.stop().await;
}

#[tokio::test]
async fn playback_feedback_honours_the_advertised_features() {
    let mut opts = Options::new(policy(|_| Mode::Accept));
    opts.welcome = Arc::new(|_, _| {
        format!(
            r#"{{"action":"welcome","protocol_version":3,"node":"node1","features":["{}"]}}"#,
            feature::PLAYBACK_FEEDBACK
        )
    });
    let server = Server::start(opts).await;
    let t = connected(&server).await;
    let _ = resolve(&t.client, "r1", Duration::from_secs(10)).await;
    let fb = |kind: &'static str| Feedback {
        url: MEDIA.to_owned(),
        kind,
        ms_since_open: 1_200,
        delivered_height: Some(720),
        detail: Some("ignored".to_owned()),
        correlation_id: None,
    };

    t.client.send_playback_feedback(fb(feedback::CACHE_PLAY));
    t.client.send_playback_feedback(fb(feedback::PLAYING));
    wait_until(
        || {
            server.record(|r| {
                r.texts
                    .iter()
                    .any(|(_, v)| v["action"] == action::PLAYBACK_FEEDBACK)
            })
        },
        5_000,
        "feedback frame",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let frames: Vec<Value> = server.record(|r| {
        r.texts
            .iter()
            .filter(|(_, v)| v["action"] == action::PLAYBACK_FEEDBACK)
            .map(|(_, v)| v.clone())
            .collect()
    });
    assert_eq!(frames.len(), 1, "{frames:?}");
    let f = &frames[0];
    assert_eq!(f["kind"], feedback::PLAYING);
    assert_eq!(f["delivered_height"], 720);
    assert_eq!(f["ms_since_open"], 1_200);
    assert_eq!(f["correlation_id"], "r1");
    assert!(f.get("detail").is_none());
    assert_eq!(f["client_id"].as_str(), Some(t.client.client_id()));
    t.client.stop().await;
}
