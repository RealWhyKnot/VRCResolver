use super::*;

struct ReceivedResolve(MeshResult);

impl Mesh for ReceivedResolve {
    fn current_node_host(&self) -> String {
        "us1.vrcresolver.com".to_owned()
    }
    fn is_connected(&self) -> bool {
        true
    }
    async fn resolve(&self, _: &ResolveRequest, _: Instant) -> Result<MeshResult, String> {
        Ok(self.0.clone())
    }
    fn send_playback_feedback(&self, _: crate::ipc::Feedback) {}
}

#[tokio::test]
async fn fallback_public_message_reaches_one_ipc_outcome_and_resolve_logs_stay_in_file() {
    use tokio::io::AsyncWriteExt;
    for binary in [false, true] {
        let harness = Harness::new();
        let mut pending = harness.pending("r1");
        let message = "\x1b[31mThe\x1b[0m\t video\n is private.\u{2603}";
        if binary {
            harness
                .binary(&mp_frame(vec![
                    mp_str(action::FALLBACK_NATIVE),
                    mp_str("r1"),
                    mp_str(fallback::ALL_CONFIGS_FAILED),
                    mp_str(message),
                ]))
                .await;
        } else {
            harness
                .json(
                    &serde_json::json!({
                        "action": action::FALLBACK_NATIVE, "id": "r1",
                        "reason": fallback::ALL_CONFIGS_FAILED, "public_message": message,
                    })
                    .to_string(),
                )
                .await;
        }
        harness
            .json(r#"{"action":"resolve_log","id":"r1","message":"trying extractor_only"}"#)
            .await;
        let result = done(&mut pending).expect("fallback");
        let ipc = crate::ipc::Ipc::builder(
            Arc::new(ReceivedResolve(result)),
            harness.client.inner.ux.clone(),
            Log::to_file(&harness.log_path),
        )
        .build();
        let (mut client, mut server) = tokio::io::duplex(65536);
        client.write_all(b"{\"action\":\"resolve\",\"id\":\"r1\",\"url\":\"https://youtube.com/watch?v=secret\",\"player\":\"avpro\"}\n").await.expect("request");
        ipc.handle(&mut server).await;
        assert!(
            shim_reply(client, server)
                .await
                .get("public_message")
                .is_none()
        );
        let text = harness.out.text();
        assert_eq!(text.matches("Skipped").count(), 1, "{text}");
        assert!(
            text.contains("The video is private. VRChat will try it on its own."),
            "{text}"
        );
        assert!(text.is_ascii());
        assert!(!text.contains("extractor_only"));
        assert!(!text.contains("[resolve_log]"));
        assert!(!text.contains("secret"));
        assert!(
            harness
                .log()
                .contains("[resolve_log] id=r1 level=info phase=trying config=extractor_only")
        );
    }
}

#[tokio::test]
async fn spotify_match_stays_in_the_console_on_fresh_and_cached_wire_replies() {
    use tokio::io::AsyncWriteExt;
    let message = "Matched on YouTube: girlhouse - worth it";
    let playback_url = "https://example.test/playback";
    for (binary, public_message) in [(false, Some(message)), (true, Some(message)), (false, None)] {
        let harness = Harness::new();
        let mut pending = harness.pending("r1");
        if binary {
            let mut fields = vec![mp_str(action::RESOLVED), mp_str("r1"), mp_str(playback_url)];
            fields.resize(16, rmpv::Value::Nil);
            fields[12] = rmpv::Value::from(720);
            fields.push(mp_str(message));
            harness.binary(&mp_frame(fields)).await;
        } else {
            harness
                .json(
                    &serde_json::json!({
                        "action": action::RESOLVED, "id": "r1", "url": playback_url,
                        "resolved_height": 720, "public_message": public_message,
                        "extra_field": {"example": true},
                    })
                    .to_string(),
                )
                .await;
        }
        let result = done(&mut pending).expect("resolved");
        let cache = Arc::new(crate::resolve_cache::ResolveCache::new(
            harness.dir.path().join("resolve_cache.json"),
            Arc::new(SystemTime::now),
            Log::disabled(),
        ));
        let ipc = crate::ipc::Ipc::builder(
            Arc::new(ReceivedResolve(result)),
            harness.client.inner.ux.clone(),
            Log::to_file(&harness.log_path),
        )
        .cache(cache)
        .build();
        for id in ["r1", "r2"] {
            let (mut client, mut server) = tokio::io::duplex(65536);
            let request = format!(
                "{{\"action\":\"resolve\",\"id\":\"{id}\",\"url\":\"https://open.spotify.com/track/test\",\"player\":\"avpro\"}}\n"
            );
            client.write_all(request.as_bytes()).await.expect("request");
            ipc.handle(&mut server).await;
            let reply = shim_reply(client, server).await;
            assert_eq!(reply["id"], id);
            assert_eq!(reply["url"], playback_url);
            assert_eq!(reply["resolved_height"], 720);
            assert!(reply.get("public_message").is_none());
            if !binary {
                assert_eq!(reply["extra_field"], serde_json::json!({"example": true}));
            }
        }
        let text = harness.out.text();
        assert_eq!(text.matches("Playing").count(), 2, "{text}");
        let shown = public_message.unwrap_or("Matched Spotify track on YouTube");
        assert_eq!(
            text.matches(&format!("{shown} (720p,")).count(),
            2,
            "{text}"
        );
        assert!(harness.log().contains("[resolve-cache] hit id=r2"));
    }
}

async fn shim_reply(mut client: tokio::io::DuplexStream, server: tokio::io::DuplexStream) -> Value {
    use tokio::io::AsyncReadExt;
    drop(server);
    let mut frame = Vec::new();
    client.read_to_end(&mut frame).await.expect("shim reply");
    serde_json::from_slice(&frame).expect("shim response JSON")
}
