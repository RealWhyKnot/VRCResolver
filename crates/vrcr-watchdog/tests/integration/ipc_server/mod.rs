use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::common::ManualClock;
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use vrcr_shared::console::Ux;
use vrcr_shared::log::Log;
use vrcr_shared::logutil::iso8601_utc;
use vrcr_shared::wire::{
    Player, ResolveRequest, ResolveResponse, action, fallback, feedback, og_reason,
};
use vrcr_watchdog::health::{self, HealthGate, Transition};
use vrcr_watchdog::ipc::server::{
    FULL_WINDOW_BUDGET_MS, effective_budget_ms, is_healthy_outcome, og_failed_detail_for,
};
use vrcr_watchdog::ipc::{Feedback, Ipc, Mesh, MeshResult, framing};
use vrcr_watchdog::og_hint::OgFallbackHint;
use vrcr_watchdog::resolve_cache::{Key, ResolveCache};

const URL: &str = "https://example.com/v";
const NODE: &str = "us1.vrcresolver.com";
const CACHED_MEDIA: &str = "https://cache.example/v.mp4";
const MESH_MEDIA: &str = "https://media.example/v.mp4";

#[derive(Clone)]
enum Step {
    Reply {
        action: &'static str,
        reason: Option<&'static str>,
        url: Option<&'static str>,
        extra: Option<(&'static str, &'static str)>,
        retry_after_ms: Option<i32>,
    },
    Hang,
    Fail,
}

fn resolved_step() -> Step {
    Step::Reply {
        action: action::RESOLVED,
        reason: None,
        url: Some(MESH_MEDIA),
        extra: None,
        retry_after_ms: None,
    }
}

fn fallback_step(reason: &'static str) -> Step {
    Step::Reply {
        action: action::FALLBACK_NATIVE,
        reason: Some(reason),
        url: None,
        extra: None,
        retry_after_ms: None,
    }
}

#[derive(Default)]
struct FakeMesh {
    connected: bool,
    script: Mutex<VecDeque<Step>>,
    repeat: Mutex<Option<Step>>,
    resolves: Mutex<Vec<ResolveRequest>>,
    feedback: Mutex<Vec<Feedback>>,
    started: tokio::sync::Notify,
}

impl FakeMesh {
    fn connected(steps: Vec<Step>) -> Arc<FakeMesh> {
        Arc::new(FakeMesh {
            connected: true,
            script: Mutex::new(steps.into()),
            ..FakeMesh::default()
        })
    }

    fn always(step: Step) -> Arc<FakeMesh> {
        let m = FakeMesh::connected(Vec::new());
        *m.repeat.lock().unwrap_or_else(|e| e.into_inner()) = Some(step);
        m
    }

    fn offline() -> Arc<FakeMesh> {
        Arc::new(FakeMesh::default())
    }

    fn resolves(&self) -> Vec<ResolveRequest> {
        self.resolves
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn feedback(&self) -> Vec<Feedback> {
        self.feedback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Mesh for FakeMesh {
    fn current_node_host(&self) -> String {
        NODE.to_owned()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn resolve(
        &self,
        req: &ResolveRequest,
        _deadline: Instant,
    ) -> Result<MeshResult, String> {
        self.resolves
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.clone());
        self.started.notify_one();
        let step = if !self.connected {
            fallback_step(fallback::SERVER_UNREACHABLE)
        } else {
            let next = self
                .script
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front();
            next.or_else(|| {
                self.repeat
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            })
            .unwrap_or_else(|| fallback_step(fallback::SERVER_UNREACHABLE))
        };
        match step {
            Step::Hang => std::future::pending().await,
            Step::Fail => Err("boom".to_owned()),
            Step::Reply {
                action,
                reason,
                url,
                extra,
                retry_after_ms,
            } => {
                let mut resp = ResolveResponse {
                    action: action.to_owned(),
                    id: req.id.clone(),
                    reason: reason.map(str::to_owned),
                    url: url.map(str::to_owned),
                    retry_after_ms,
                    ..ResolveResponse::default()
                };
                if let Some((k, v)) = extra {
                    resp.extra.insert(k.to_owned(), Value::String(v.to_owned()));
                }
                Ok(MeshResult {
                    frame: serde_json::to_vec(&resp).unwrap_or_default(),
                    action: action.to_owned(),
                    reason: reason.map(str::to_owned),
                    retry_after_ms,
                })
            }
        }
    }

    fn send_playback_feedback(&self, f: Feedback) {
        self.feedback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(f);
    }
}

async fn exchange(ipc: &Ipc<FakeMesh>, line: &str) -> String {
    let (mut client, mut server) = tokio::io::duplex(1 << 20);
    client
        .write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    ipc.handle(&mut server).await;
    drop(server);
    let mut out = String::new();
    client
        .read_to_string(&mut out)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    out
}

async fn ask(ipc: &Ipc<FakeMesh>, line: &str) -> Value {
    let out = exchange(ipc, line).await;
    let mut lines = out.lines();
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("no reply for {line}"));
    assert!(lines.next().is_none(), "more than one reply: {out}");
    serde_json::from_str(first).unwrap_or_else(|e| panic!("{e}: {first}"))
}

fn req(player: &str, extra: &str) -> String {
    format!(
        "{{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"{URL}\",\"player\":\"{player}\"{extra}}}"
    )
}

fn open_gate(now: vrcr_watchdog::clock::Now) -> Arc<HealthGate> {
    let g = HealthGate::new(
        health::DEFAULT_COOLDOWN,
        health::DEFAULT_VERDICT_TIMEOUT,
        now,
    );
    for _ in 0..health::OPEN_THRESHOLD {
        g.record_resolve_outcome(vrcr_watchdog::health::ResolveOutcome {
            healthy: false,
            resolved: false,
        });
    }
    Arc::new(g)
}

fn seeded_cache(dir: &TempDir) -> Arc<ResolveCache> {
    let clock = ManualClock::at(SystemTime::now());
    let cache = ResolveCache::new(
        dir.path().join("resolve_cache.json"),
        clock.now(),
        Log::disabled(),
    );
    cache.store(
        &Key {
            node: NODE,
            url: URL,
            player: Some("unity"),
            format_arg: None,
            max_height: Some(720),
        },
        &ResolveResponse {
            action: action::RESOLVED.to_owned(),
            url: Some(CACHED_MEDIA.to_owned()),
            expires_at: Some(iso8601_utc(SystemTime::now() + Duration::from_secs(3600))),
            ..ResolveResponse::default()
        },
    );
    Arc::new(cache)
}

const UNITY_720: &str = "{\"action\":\"resolve\",\"id\":\"probe1\",\"url\":\"https://example.com/v\",\"player\":\"unity\",\"maxHeight\":720}";

fn reason(v: &Value) -> &str {
    v["reason"].as_str().unwrap_or_default()
}

#[cfg(windows)]
mod pipe {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::windows::named_pipe::ClientOptions;
    use vrcr_watchdog::ipc::pipe::{current_user_sid, pipe_sddl};

    #[test]
    fn sddl_grants_the_user_and_system_and_labels_the_pipe_low() {
        let sid = current_user_sid().unwrap_or_else(|e| panic!("{e}"));
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(
            pipe_sddl("S-1-5-21-1"),
            "O:S-1-5-21-1G:S-1-5-21-1D:(A;;0x1f019f;;;S-1-5-21-1)(A;;0x1f019f;;;SY)S:(ML;;NW;;;LW)"
        );
    }

    async fn connect(name: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
        let path = format!(r"\\.\pipe\{name}");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(client) = ClientOptions::new().open(&path) {
                    return client;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pipe never came up")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_limit_disconnects_and_warns_once_for_repeated_rejections() {
        let name = format!("vrcresolver.test.{}", uuid::Uuid::new_v4().simple());
        let directory = TempDir::new().expect("temporary log directory");
        let path = directory.path().join("ipc.log");
        let log = Log::to_file(&path);
        let mesh = FakeMesh::always(Step::Hang);
        let ipc = Ipc::builder(Arc::clone(&mesh), Ux::sink(), log.clone()).build();
        let server = ipc.serve(&name);
        let mut clients = Vec::new();
        for _ in 0..128 {
            let started = mesh.started.notified();
            let mut client = connect(&name).await;
            client
                .write_all(format!("{}\n", req("avpro", "")).as_bytes())
                .await
                .expect("request written");
            started.await;
            clients.push(client);
        }
        for _ in 0..2 {
            let mut excess = connect(&name).await;
            let mut byte = [0];
            let result = tokio::time::timeout(Duration::from_secs(2), excess.read(&mut byte))
                .await
                .expect("rejected pipe disconnects");
            assert!(matches!(result, Ok(0) | Err(_)), "{result:?}");
        }
        server.abort();
        let _ = server.await;
        log.close();
        let text = std::fs::read_to_string(path).expect("IPC log");
        assert_eq!(text.matches("client limit reached").count(), 1, "{text}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_named_pipe_serves_concurrent_clients() {
        let name = format!("vrcresolver.test.{}", uuid::Uuid::new_v4().simple());
        let mesh = FakeMesh::always(resolved_step());
        let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled()).build();
        let server = ipc.serve(&name);

        let mut tasks = Vec::new();
        for i in 0..4 {
            let name = name.clone();
            tasks.push(tokio::spawn(async move {
                let client = connect(&name).await;
                let mut reader = BufReader::new(client);
                let line = if i == 0 {
                    "{not json".to_owned()
                } else {
                    format!(
                        "{{\"action\":\"resolve\",\"id\":\"p{i}\",\"url\":\"{URL}\",\"player\":\"avpro\"}}"
                    )
                };
                reader
                    .get_mut()
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .unwrap_or_else(|e| panic!("{e}"));
                let mut reply = String::new();
                reader
                    .read_line(&mut reply)
                    .await
                    .unwrap_or_else(|e| panic!("{e}"));
                let mut rest = String::new();
                let _ = reader.read_line(&mut rest).await;
                (i, reply, rest)
            }));
        }
        for t in tasks {
            let (i, reply, rest) = t.await.unwrap_or_else(|e| panic!("{e}"));
            let v: Value = serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"));
            if i == 0 {
                assert_eq!(reason(&v), fallback::INTERNAL_ERROR);
            } else {
                assert_eq!(v["url"], MESH_MEDIA);
                assert_eq!(v["id"], format!("p{i}"));
            }
            assert_eq!(rest, "", "server should hang up after one reply");
        }
        assert_eq!(mesh.resolves().len(), 3);
        server.abort();
    }

    #[tokio::test]
    async fn stopping_the_pipe_server_cancels_its_active_clients() {
        let name = format!("vrcresolver.test.{}", uuid::Uuid::new_v4().simple());
        let mesh = FakeMesh::always(Step::Hang);
        let ipc = Ipc::builder(Arc::clone(&mesh), Ux::sink(), Log::disabled()).build();
        let server = ipc.serve(&name);
        let mut client = connect(&name).await;
        client.write_all(format!("{{\"action\":\"resolve\",\"id\":\"cancel\",\"url\":\"{URL}\",\"player\":\"avpro\"}}\n").as_bytes())
            .await.expect("local pipe accepts the request");
        mesh.started.notified().await;
        server.abort();
        assert!(
            server
                .await
                .expect_err("server was cancelled")
                .is_cancelled()
        );
        let mut byte = [0];
        let result = tokio::time::timeout(Duration::from_secs(2), client.read(&mut byte))
            .await
            .expect("stopping the listener must close the active pipe");
        assert!(matches!(result, Ok(0) | Err(_)), "{result:?}");
    }
}

mod cache_cases;

mod contracts_cases;

mod events_cases;

mod health_cases;

mod local_cases;

mod requests_cases;
