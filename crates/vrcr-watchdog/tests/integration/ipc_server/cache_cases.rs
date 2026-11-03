use super::*;

#[tokio::test]
async fn resolved_mesh_answers_are_cached_for_the_next_ask() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cache = Arc::new(ResolveCache::new(
        dir.path().join("resolve_cache.json"),
        vrcr_watchdog::clock::system(),
        Log::disabled(),
    ));
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .cache(std::sync::Arc::clone(&cache))
        .build();
    assert_eq!(ask(&ipc, UNITY_720).await["url"], MESH_MEDIA);
    assert_eq!(cache.count(), 1);
    let again = ask(&ipc, &UNITY_720.replace("probe1", "second")).await;
    assert_eq!(again["url"], MESH_MEDIA);
    assert_eq!(again["id"], "second");
    assert_eq!(mesh.resolves().len(), 1);
}
