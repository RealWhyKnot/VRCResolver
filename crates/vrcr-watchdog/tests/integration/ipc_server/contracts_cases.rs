use super::*;

#[tokio::test]
async fn skip_native_hint_never_clears_an_active_hint() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(URL);
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .og_hint(std::sync::Arc::clone(&hint))
        .build();
    let v = ask(&ipc, &req("avpro", ",\"skip_native_hint\":true")).await;
    assert_eq!(reason(&v), og_reason::PRIOR_LOAD_FAILURE);
    assert_eq!(hint.live_entry_count(), 1);
    assert_eq!(mesh.resolves().len(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_blocked_og_lets_our_resolver_retry_a_hinted_link() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure("https://youtu.be/abcdefghijk");
    hint.record_og_blocked("https://youtu.be/abcdefghijk");
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .og_hint(hint)
        .build();
    let line = "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://www.youtube.com/watch?v=abcdefghijk&si=x\",\"player\":\"avpro\"}";
    assert_eq!(ask(&ipc, line).await["url"], MESH_MEDIA);
    assert_eq!(mesh.resolves().len(), 1);
}
