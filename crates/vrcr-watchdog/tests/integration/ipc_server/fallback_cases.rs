use super::*;
use vrcr_watchdog::emitted::EmittedUrls;

const PROXY: &str = "https://vrcresolver.com/api/proxy/manifest.m3u8?q=abc";

fn temp_cache(dir: &TempDir) -> Arc<ResolveCache> {
    Arc::new(ResolveCache::new(
        dir.path().join("resolve_cache.json"),
        vrcr_watchdog::clock::system(),
        Log::disabled(),
    ))
}

fn live_step(validated: bool) -> Step {
    Step::Raw(Box::new(ResolveResponse {
        action: action::RESOLVED.to_owned(),
        url: Some(MESH_MEDIA.to_owned()),
        live: Some(true),
        delivery: Some("proxy".to_owned()),
        validated: Some(validated),
        ..ResolveResponse::default()
    }))
}

fn resolve_line(id: &str, url: &str) -> String {
    format!("{{\"action\":\"resolve\",\"id\":\"{id}\",\"url\":\"{url}\",\"player\":\"avpro\"}}")
}

#[tokio::test]
async fn an_unvalidated_live_answer_is_played_but_never_cached() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = temp_cache(&dir);
    let mesh = FakeMesh::connected(vec![live_step(false), live_step(false)]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(std::sync::Arc::clone(&cache))
        .build();
    let first = ask(&ipc, UNITY_720).await;
    assert_eq!(first["url"], MESH_MEDIA);
    assert_eq!(first["live"], true);
    assert_eq!(first["validated"], false);
    assert_eq!(cache.count(), 0);
    ask(&ipc, &UNITY_720.replace("probe1", "second")).await;
    assert_eq!(mesh.resolves().len(), 2);
}

#[tokio::test]
async fn a_validated_live_answer_is_cached() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = temp_cache(&dir);
    let mesh = FakeMesh::connected(vec![live_step(true)]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(std::sync::Arc::clone(&cache))
        .build();
    assert_eq!(ask(&ipc, UNITY_720).await["url"], MESH_MEDIA);
    assert_eq!(cache.count(), 1);
}

#[tokio::test]
async fn a_cached_answer_is_never_served_while_the_failure_hint_is_active() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = seeded_cache(&dir);
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(URL);
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(std::sync::Arc::clone(&cache))
        .og_hint(hint)
        .build();
    let v = ask(&ipc, UNITY_720).await;
    assert_eq!(reason(&v), og_reason::PRIOR_LOAD_FAILURE);
    assert!(v["url"].is_null());
    assert_eq!(mesh.resolves().len(), 0);
}

#[tokio::test]
async fn first_party_playback_urls_pass_through_and_are_remembered() {
    let emitted = Arc::new(EmittedUrls::default());
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .emitted(std::sync::Arc::clone(&emitted))
        .build();
    let v = ask(&ipc, &resolve_line("fp1", PROXY)).await;
    assert_eq!(v["action"], action::RESOLVED);
    assert_eq!(v["url"], PROXY);
    assert_eq!(v["id"], "fp1");
    assert_eq!(mesh.resolves().len(), 0);
    let origin = emitted
        .origin_by_url(PROXY)
        .unwrap_or_else(|| panic!("remembered"));
    assert_eq!(origin.source_url, PROXY);
    assert_eq!(origin.correlation_id.as_deref(), Some("fp1"));
}

#[tokio::test]
async fn first_party_playback_urls_pass_through_despite_a_hint_or_a_paused_gate() {
    for url in [PROXY, "https://vrcresolver.com/api/proxy?q=abc"] {
        let hint = Arc::new(OgFallbackHint::default());
        hint.record_load_failure(url);
        let mesh = FakeMesh::connected(vec![]);
        let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
            .og_hint(hint)
            .health(open_gate(vrcr_watchdog::clock::system()))
            .build();
        let v = ask(&ipc, &resolve_line("fp2", url)).await;
        assert_eq!(v["action"], action::RESOLVED, "{url}");
        assert_eq!(v["url"], url);
        assert!(v["reason"].is_null(), "{v}");
        assert_eq!(mesh.resolves().len(), 0);
    }
}

#[tokio::test]
async fn an_og_blocked_source_is_resolved_even_while_the_gate_is_paused() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(URL);
    hint.record_og_blocked(URL);
    let gate = open_gate(vrcr_watchdog::clock::system());
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .og_hint(std::sync::Arc::clone(&hint))
        .health(std::sync::Arc::clone(&gate))
        .build();
    assert_eq!(ask(&ipc, &resolve_line("b1", URL)).await["url"], MESH_MEDIA);
    assert_eq!(mesh.resolves().len(), 1);
    let other = "https://www.youtube.com/watch?v=zzzzzzzzzzz";
    assert_eq!(
        reason(&ask(&ipc, &resolve_line("b2", other)).await),
        og_reason::RESOLVER_UNHEALTHY
    );
    assert_eq!(mesh.resolves().len(), 1);
    assert!(gate.should_short_circuit(true).0);
}

#[tokio::test]
async fn arming_records_the_hint_before_the_cache_is_evicted() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = temp_cache(&dir);
    let cached_when_hint_ran = Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));
    let hint = {
        let cache = std::sync::Arc::clone(&cache);
        let seen = std::sync::Arc::clone(&cached_when_hint_ran);
        Arc::new(OgFallbackHint::new(Arc::new(move || {
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(cache.count());
            SystemTime::now()
        })))
    };
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(std::sync::Arc::clone(&cache))
        .build();
    assert_eq!(ask(&ipc, &resolve_line("c1", URL)).await["url"], MESH_MEDIA);
    assert_eq!(cache.count(), 1);
    let (evicted, lease) = vrcr_watchdog::og_hint::arm(Some(&cache), Some(&hint), URL);
    assert_eq!((evicted, lease.is_some()), (1, true));
    let seen = cached_when_hint_ran
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert_eq!(seen.first(), Some(&1), "{seen:?}");
}

#[tokio::test]
async fn resolved_mesh_urls_map_back_to_the_request() {
    let emitted = Arc::new(EmittedUrls::default());
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .emitted(std::sync::Arc::clone(&emitted))
        .build();
    assert_eq!(ask(&ipc, &req("avpro", "")).await["url"], MESH_MEDIA);
    let origin = emitted
        .origin_by_url(MESH_MEDIA)
        .unwrap_or_else(|| panic!("remembered"));
    assert_eq!(origin.source_url, URL);
    assert_eq!(
        emitted.origin_by_id("x1").map(|o| o.source_url).as_deref(),
        Some(URL)
    );
}

#[tokio::test(start_paused = true)]
async fn cdn4_live_playlist_after_a_load_failure_goes_to_the_fallback_without_the_mesh() {
    let cdn4 = "https://live.cdn4.io/ttfcvr/index.m3u8";
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = temp_cache(&dir);
    let hint = Arc::new(OgFallbackHint::default());
    let mesh = FakeMesh::connected(vec![resolved_step(), resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(std::sync::Arc::clone(&cache))
        .og_hint(std::sync::Arc::clone(&hint))
        .build();
    assert_eq!(
        ask(&ipc, &resolve_line("a1", cdn4)).await["url"],
        MESH_MEDIA
    );
    assert_eq!(cache.count(), 1);
    let (evicted, lease) = vrcr_watchdog::og_hint::arm(Some(&cache), Some(&hint), cdn4);
    assert_eq!(evicted, 1);
    assert_eq!(lease, Some(Duration::from_secs(5 * 60)));
    assert_eq!(cache.count(), 0);
    for (id, extra) in [("a2", ""), ("a3", ",\"skip_native_hint\":true")] {
        let line = format!(
            "{{\"action\":\"resolve\",\"id\":\"{id}\",\"url\":\"{cdn4}\",\"player\":\"avpro\"{extra}}}"
        );
        assert_eq!(
            reason(&ask(&ipc, &line).await),
            og_reason::PRIOR_LOAD_FAILURE,
            "{id}"
        );
    }
    assert_eq!(mesh.resolves().len(), 1);
    assert!(hint.should_prefer_og(cdn4));
}
