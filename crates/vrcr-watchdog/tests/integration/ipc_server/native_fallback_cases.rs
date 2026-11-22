use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use vrcr_shared::console::NATIVE_FALLBACK_SENTENCE;
use vrcr_shared::native_fallback::NativeFallbackHosts;
use vrcr_watchdog::local::{self, Ask, Attempt};

use crate::common::mesh::Capture;

use super::*;

const KSYNC_URL: &str = "https://ksync.arcanescripts.com/watch/42";

#[derive(Default)]
struct CountingStep {
    calls: AtomicUsize,
}

impl local::Step for CountingStep {
    fn covers(&self, _url: &str) -> bool {
        true
    }

    fn play<'a>(&'a self, _ask: Ask<'a>) -> Attempt<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err("not_expected") })
    }
}

type Reports = Arc<Mutex<Vec<String>>>;

struct Rig {
    ipc: Arc<Ipc<FakeMesh>>,
    mesh: Arc<FakeMesh>,
    hint: Arc<OgFallbackHint>,
    gate: Arc<HealthGate>,
    local: Arc<CountingStep>,
    reports: Reports,
    out: Capture,
    _dir: TempDir,
}

fn listed(entries: &[&str]) -> Arc<RwLock<NativeFallbackHosts>> {
    Arc::new(RwLock::new(NativeFallbackHosts::new(entries)))
}

fn one_failure_from_open() -> Arc<HealthGate> {
    let gate = HealthGate::new(
        health::DEFAULT_COOLDOWN,
        health::DEFAULT_VERDICT_TIMEOUT,
        vrcr_watchdog::clock::system(),
    );
    for _ in 1..health::OPEN_THRESHOLD {
        gate.record_resolve_outcome(health::ResolveOutcome {
            healthy: false,
            resolved: false,
        });
    }
    Arc::new(gate)
}

fn rig(mesh: Arc<FakeMesh>, hosts: &[&str]) -> Rig {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let out = Capture::default();
    let hint = Arc::new(OgFallbackHint::default());
    let gate = one_failure_from_open();
    let local = Arc::new(CountingStep::default());
    let reports: Reports = Arc::default();
    let sink = Arc::clone(&reports);
    let step: Arc<dyn local::Step> = Arc::<CountingStep>::clone(&local);
    let ipc = Ipc::builder(Arc::clone(&mesh), out.ux(), Log::disabled())
        .cache(seeded_cache(&dir))
        .og_hint(Arc::clone(&hint))
        .health(Arc::clone(&gate))
        .local(Some(step))
        .report_fallback(move |_, reason| {
            sink.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(reason.to_owned());
        })
        .native_fallback(listed(hosts))
        .build();
    Rig {
        ipc,
        mesh,
        hint,
        gate,
        local,
        reports,
        out,
        _dir: dir,
    }
}

fn resolve_line(id: &str, url: &str, extra: &str) -> String {
    format!(
        "{{\"action\":\"resolve\",\"id\":\"{id}\",\"url\":\"{url}\",\"player\":\"avpro\"{extra}}}"
    )
}

fn assert_nothing_counted(r: &Rig, url: &str) {
    assert_eq!(r.local.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        *r.reports.lock().unwrap_or_else(|e| e.into_inner()),
        Vec::<String>::new()
    );
    assert_eq!(r.hint.strikes(url), 0);
    assert!(!r.hint.should_prefer_og(url));
    assert_eq!(r.gate.should_short_circuit(true), (false, Transition::None));
    assert_eq!(
        r.gate.record_resolve_outcome(health::ResolveOutcome {
            healthy: false,
            resolved: false,
        }),
        Transition::Opened
    );
}

#[tokio::test(start_paused = true)]
async fn listed_host_falls_back_natively_without_asking_the_server() {
    let r = rig(
        FakeMesh::always(resolved_step()),
        &["ksync.arcanescripts.com"],
    );

    let v = ask(&r.ipc, &resolve_line("n1", KSYNC_URL, "")).await;

    assert_eq!(v["action"], action::FALLBACK_NATIVE);
    assert_eq!(v["id"], "n1");
    assert_eq!(reason(&v), fallback::NATIVE_FALLBACK_HOST);
    assert_eq!(v.get("public_message"), None);
    assert_eq!(r.mesh.resolves().len(), 0);
    assert_eq!(r.mesh.feedback(), Vec::<Feedback>::new());
    assert_nothing_counted(&r, KSYNC_URL);
}

#[tokio::test(start_paused = true)]
async fn wrapper_re_ask_for_a_listed_host_stays_local() {
    let r = rig(
        FakeMesh::always(resolved_step()),
        &["ksync.arcanescripts.com"],
    );

    let v = ask(
        &r.ipc,
        &resolve_line("n2", KSYNC_URL, ",\"skip_native_hint\":true"),
    )
    .await;

    assert_eq!(reason(&v), fallback::NATIVE_FALLBACK_HOST);
    assert_eq!(r.mesh.resolves().len(), 0);
    assert_nothing_counted(&r, KSYNC_URL);
}

#[tokio::test(start_paused = true)]
async fn wildcard_entries_cover_subdomains_but_not_the_apex() {
    let r = rig(
        FakeMesh::always(resolved_step()),
        &["*.native.example.test"],
    );

    let sub = ask(
        &r.ipc,
        &resolve_line("w1", "https://cdn.native.example.test/a", ""),
    )
    .await;
    let apex = ask(
        &r.ipc,
        &resolve_line("w2", "https://native.example.test/a", ""),
    )
    .await;

    assert_eq!(reason(&sub), fallback::NATIVE_FALLBACK_HOST);
    assert_eq!(apex["url"], MESH_MEDIA);
    assert_eq!(r.mesh.resolves().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn look_alike_hosts_still_go_to_the_server() {
    let r = rig(
        FakeMesh::always(resolved_step()),
        &["ksync.arcanescripts.com"],
    );
    let urls = [
        "https://evilksync.arcanescripts.com/watch/42",
        "https://a.ksync.arcanescripts.com/watch/42",
        "https://ksync.arcanescripts.com.evil.com/watch/42",
    ];

    for (index, url) in urls.iter().enumerate() {
        let v = ask(&r.ipc, &resolve_line(&format!("l{index}"), url, "")).await;
        assert_eq!(v["url"], MESH_MEDIA, "{url}");
    }

    assert_eq!(r.mesh.resolves().len(), urls.len());
}

#[tokio::test(start_paused = true)]
async fn empty_list_sends_every_host_to_the_server() {
    let r = rig(FakeMesh::always(resolved_step()), &[]);

    let v = ask(&r.ipc, &resolve_line("e1", KSYNC_URL, "")).await;

    assert_eq!(v["url"], MESH_MEDIA);
    assert_eq!(r.mesh.resolves().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn listed_host_shows_one_calm_console_line() {
    let r = rig(
        FakeMesh::always(resolved_step()),
        &["ksync.arcanescripts.com"],
    );

    ask(&r.ipc, &resolve_line("c1", KSYNC_URL, "")).await;

    let console = r.out.text();
    assert_eq!(
        console.matches(NATIVE_FALLBACK_SENTENCE).count(),
        1,
        "{console}"
    );
    assert!(console.contains("Skipped"), "{console}");
    assert!(!console.contains("Failed"), "{console}");
    assert!(
        !console.contains("VRChat will try it on its own"),
        "{console}"
    );
    let logs = r.out.logs();
    assert!(!logs.contains("resolve failed"), "{logs}");
    assert!(
        logs.contains("native fallback (site blocks the server) id=c1"),
        "{logs}"
    );
}

#[tokio::test(start_paused = true)]
async fn server_native_fallback_host_reply_is_expected_and_not_retried() {
    let r = rig(
        FakeMesh::always(fallback_step(fallback::NATIVE_FALLBACK_HOST)),
        &[],
    );

    let v = ask(&r.ipc, &resolve_line("s1", KSYNC_URL, "")).await;

    assert_eq!(v["action"], action::FALLBACK_NATIVE);
    assert_eq!(v["id"], "s1");
    assert_eq!(reason(&v), fallback::NATIVE_FALLBACK_HOST);
    assert_eq!(r.mesh.resolves().len(), 1);
    assert_nothing_counted(&r, KSYNC_URL);
    assert_eq!(r.out.text().matches(NATIVE_FALLBACK_SENTENCE).count(), 1);
}

#[tokio::test(start_paused = true)]
async fn other_server_fallbacks_still_count_against_health() {
    let r = rig(
        FakeMesh::always(fallback_step(fallback::SERVER_UNREACHABLE)),
        &["ksync.arcanescripts.com"],
    );

    ask(&r.ipc, &resolve_line("o1", URL, "")).await;

    assert!(r.gate.should_short_circuit(true).0);
}
