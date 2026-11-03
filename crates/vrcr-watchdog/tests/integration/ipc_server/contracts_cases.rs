use super::*;

#[tokio::test]
async fn skip_native_hint_clears_the_hint_and_asks_the_mesh() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure(URL);
    let mesh = FakeMesh::connected(vec![resolved_step()]);
    let ipc = Ipc::builder(std::sync::Arc::clone(&mesh), Ux::sink(), Log::disabled())
        .og_hint(std::sync::Arc::clone(&hint))
        .build();
    let v = ask(&ipc, &req("avpro", ",\"skip_native_hint\":true")).await;
    assert_eq!(v["url"], MESH_MEDIA);
    assert_eq!(hint.live_entry_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn prior_load_failure_does_not_send_a_blocked_site_to_og() {
    let hint = Arc::new(OgFallbackHint::default());
    hint.record_load_failure("https://youtu.be/abc");
    hint.record_og_blocked("https://youtu.be/abc");
    let ipc = Ipc::builder(FakeMesh::offline(), Ux::sink(), Log::disabled())
        .og_hint(hint)
        .build();
    let line = "{\"action\":\"resolve\",\"id\":\"x1\",\"url\":\"https://youtu.be/abc\",\"player\":\"avpro\"}";
    assert_eq!(reason(&ask(&ipc, line).await), fallback::SERVER_UNREACHABLE);
}
