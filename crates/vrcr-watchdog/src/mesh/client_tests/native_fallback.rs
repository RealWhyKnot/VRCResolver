use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vrcr_shared::native_fallback::MAX_ENTRIES;

use super::*;

fn native_hosts(h: &Harness) -> Vec<String> {
    h.client
        .native_fallback()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .entries()
        .to_vec()
}

fn blocks(h: &Harness, url: &str) -> bool {
    h.client
        .native_fallback()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .matches_url(url)
}

#[tokio::test]
async fn full_welcome_sets_the_native_fallback_hosts() {
    let h = Harness::v3(false);

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":["welcome_native_fallback"],"welcome_hash":"h1","node_region":"eu","native_fallback_hosts":["*.native.example.test","ksync.arcanescripts.com"]}"#)
        .await;

    assert_eq!(
        native_hosts(&h),
        ["*.native.example.test", "ksync.arcanescripts.com"].map(str::to_owned)
    );
    assert!(blocks(&h, "https://ksync.arcanescripts.com/watch/1"));
    assert!(blocks(&h, "https://cdn.native.example.test/a.mp4"));
    assert!(!blocks(&h, "https://evilksync.arcanescripts.com/watch/1"));
    let entry = h
        .client
        .inner
        .welcome_cache
        .get(PROXY_HOST)
        .unwrap_or_else(|| panic!("not cached"));
    assert_eq!(
        entry.native_fallback_hosts,
        Some(
            ["*.native.example.test", "ksync.arcanescripts.com"]
                .map(str::to_owned)
                .to_vec()
        )
    );
}

#[tokio::test]
async fn full_welcome_without_native_fallback_hosts_clears_the_list() {
    let h = Harness::v3(false);
    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h1","native_fallback_hosts":["ksync.arcanescripts.com"]}"#)
        .await;
    assert!(blocks(&h, "https://ksync.arcanescripts.com/"));

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h2","node_region":"eu"}"#)
        .await;

    assert_eq!(native_hosts(&h), Vec::<String>::new());
    assert!(!blocks(&h, "https://ksync.arcanescripts.com/"));
}

#[tokio::test]
async fn legacy_welcome_clears_the_native_fallback_hosts() {
    let h = Harness::new();
    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"native_fallback_hosts":["ksync.arcanescripts.com"]}"#)
        .await;
    assert!(blocks(&h, "https://ksync.arcanescripts.com/"));

    h.json(r#"{"action":"welcome","protocol_version":1,"node":"fake"}"#)
        .await;

    assert!(!blocks(&h, "https://ksync.arcanescripts.com/"));
}

#[tokio::test]
async fn cached_welcome_restores_the_native_fallback_hosts() {
    let h = Harness::v3(true);
    assert!(!blocks(&h, "https://ksync.arcanescripts.com/"));

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"node":"node1","negotiated_format":"msgpack","node_region":"eu"}"#)
        .await;

    assert_eq!(native_hosts(&h), vec!["ksync.arcanescripts.com".to_owned()]);
    assert!(blocks(&h, "https://ksync.arcanescripts.com/"));
}

#[tokio::test]
async fn cached_welcome_keeps_the_list_from_the_last_full_welcome() {
    let h = Harness::v3(false);
    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h1","native_fallback_hosts":["*.native.example.test"]}"#)
        .await;

    h.json(r#"{"action":"welcome_cached","protocol_version":3,"node":"node1"}"#)
        .await;

    assert_eq!(native_hosts(&h), vec!["*.native.example.test".to_owned()]);
}

#[tokio::test]
async fn unparseable_welcome_clears_the_native_fallback_hosts() {
    let h = Harness::v3(false);
    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"native_fallback_hosts":["ksync.arcanescripts.com"]}"#)
        .await;
    assert!(blocks(&h, "https://ksync.arcanescripts.com/"));

    h.json(r#"{"action":"welcome","protocol_version":"three"}"#)
        .await;

    assert!(!blocks(&h, "https://ksync.arcanescripts.com/"));
}

#[tokio::test]
async fn oversized_wire_list_is_capped() {
    let h = Harness::v3(false);
    let hosts: Vec<String> = (0..=MAX_ENTRIES)
        .map(|index| format!("h{index:04}.test"))
        .collect();
    let frame = serde_json::json!({
        "action": action::WELCOME, "protocol_version": 3, "node": "node1",
        "features": [], "welcome_hash": "big", "native_fallback_hosts": hosts,
    });

    h.json(&frame.to_string()).await;

    assert_eq!(native_hosts(&h).len(), MAX_ENTRIES);
    assert!(!blocks(&h, &format!("https://{}/", hosts[MAX_ENTRIES])));
}

#[derive(Default)]
struct CountingMesh(AtomicUsize);

impl Mesh for CountingMesh {
    fn current_node_host(&self) -> String {
        PROXY_HOST.to_owned()
    }
    fn is_connected(&self) -> bool {
        true
    }
    async fn resolve(&self, req: &ResolveRequest, _: Instant) -> Result<MeshResult, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let response = ResolveResponse {
            action: action::RESOLVED.to_owned(),
            id: req.id.clone(),
            url: Some("https://cdn.example.org/v.m3u8".to_owned()),
            ..ResolveResponse::default()
        };
        Ok(MeshResult {
            frame: serde_json::to_vec(&response)
                .map_err(|e| format!("encoding the resolved frame failed: {e}"))?,
            action: action::RESOLVED.to_owned(),
            reason: None,
            retry_after_ms: None,
        })
    }
    fn send_playback_feedback(&self, _: crate::ipc::Feedback) {}
}

async fn ask_ipc(ipc: &crate::ipc::Ipc<CountingMesh>, id: &str) -> Value {
    let (mut client, mut server) = tokio::io::duplex(65536);
    let line = format!(
        "{{\"action\":\"resolve\",\"id\":\"{id}\",\"url\":\"https://ksync.arcanescripts.com/watch/7\",\"player\":\"avpro\"}}\n"
    );
    client
        .write_all(line.as_bytes())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    ipc.handle(&mut server).await;
    drop(server);
    let mut out = String::new();
    client
        .read_to_string(&mut out)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"))
}

#[tokio::test]
async fn welcome_list_reaches_the_ipc_fast_path_and_a_later_welcome_lifts_it() {
    let h = Harness::v3(false);
    let mesh = Arc::new(CountingMesh::default());
    let ipc = crate::ipc::Ipc::builder(
        Arc::clone(&mesh),
        h.client.inner.ux.clone(),
        Log::disabled(),
    )
    .native_fallback(h.client.native_fallback())
    .build();

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":["welcome_native_fallback"],"welcome_hash":"h1","native_fallback_hosts":["ksync.arcanescripts.com"]}"#)
        .await;
    let skipped = ask_ipc(&ipc, "f1").await;

    assert_eq!(skipped["action"], action::FALLBACK_NATIVE);
    assert_eq!(skipped["reason"], fallback::NATIVE_FALLBACK_HOST);
    assert_eq!(mesh.0.load(Ordering::SeqCst), 0);

    h.json(r#"{"action":"welcome","protocol_version":3,"node":"node1","features":[],"welcome_hash":"h2"}"#)
        .await;
    let resolved = ask_ipc(&ipc, "f2").await;

    assert_eq!(resolved["action"], action::RESOLVED);
    assert_eq!(mesh.0.load(Ordering::SeqCst), 1);
}
