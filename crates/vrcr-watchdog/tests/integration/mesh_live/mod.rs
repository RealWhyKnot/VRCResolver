use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::mesh::{
    Conn, MEDIA, Mode, Options, Server, client_for, client_in, fixed_backoff, handler, policy,
    wait_until,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use vrcr_shared::console::Ux;
use vrcr_shared::log::Log;
use vrcr_shared::wire::{ResolveRequest, action, fallback, feature, feedback};
use vrcr_watchdog::ipc::{Feedback, Ipc, Mesh, MeshResult};
use vrcr_watchdog::mesh::{MeshClient, PROXY_HOST, connect::Phase};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn request(id: &str) -> ResolveRequest {
    ResolveRequest {
        id: id.to_owned(),
        url: "https://www.youtube.com/watch?v=abc".to_owned(),
        player: Some(vrcr_shared::wire::Player::AvPro),
        ..ResolveRequest::default()
    }
}

async fn resolve(client: &MeshClient, id: &str, within: Duration) -> MeshResult {
    client
        .resolve(&request(id), Instant::now() + within)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

fn probing(
    server: &Server,
    probe_after: Duration,
    pong_deadline: Duration,
) -> crate::common::mesh::TestClient {
    client_for(server, |c| {
        c.stall_probe_after = probe_after;
        c.stall_pong_deadline = pong_deadline;
    })
}

fn sent_deadline(json: &str) -> i64 {
    let v: Value = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
    v["wrapper_deadline_ms"]
        .as_i64()
        .unwrap_or_else(|| panic!("no deadline in {json}"))
}

async fn connected(server: &Server) -> crate::common::mesh::TestClient {
    let t = client_for(server, |_| {});
    t.client.start();
    wait_until(|| t.client.is_connected(), 5_000, "mesh connected").await;
    wait_until(|| t.client.server_protocol_version() > 0, 5_000, "welcome").await;
    t
}

async fn ipc_round_trip(client: &MeshClient, line: &str) -> Value {
    let ipc = Ipc::builder(Arc::new(client.clone()), Ux::sink(), Log::disabled()).build();
    let (mut a, mut b) = tokio::io::duplex(1 << 20);
    a.write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    ipc.handle(&mut b).await;
    drop(b);
    let mut out = String::new();
    a.read_to_string(&mut out)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let first = out.lines().next().unwrap_or_else(|| panic!("no reply"));
    serde_json::from_str(first).unwrap_or_else(|e| panic!("{e}: {first}"))
}

fn request_line(wrapper_deadline_ms: Option<i32>) -> String {
    let extra =
        wrapper_deadline_ms.map_or_else(String::new, |ms| format!(",\"wrapper_deadline_ms\":{ms}"));
    format!(
        "{{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://example.com/v\",\"player\":\"avpro\"{extra}}}"
    )
}

fn v3_welcome(msgpack: bool) -> crate::common::mesh::Welcome {
    Arc::new(move |_, hello: Option<&Value>| {
        let hello = hello.cloned().unwrap_or(Value::Null);
        let format = if msgpack { "msgpack" } else { "json" };
        if hello["welcome_hash"] == "hash1" {
            format!(
                r#"{{"action":"welcome_cached","protocol_version":3,"node":"node1","negotiated_format":"{format}"}}"#
            )
        } else {
            format!(
                r#"{{"action":"welcome","protocol_version":3,"node":"node1","server_version":"s1","features":["{}","{}","{}"],"welcome_hash":"hash1","negotiated_format":"{format}"}}"#,
                feature::PLAYBACK_FEEDBACK,
                feature::PLAYBACK_FEEDBACK_V2,
                feature::WELCOME_HOSTS
            )
        }
    })
}

mod contracts_cases;

mod deadlines_cases;

mod negotiation_cases;

mod probes_cases;

mod reconnect_cases;
