use vrcr_ytdlp::log::ShimLog;

#[test]
fn wrapper_logs_redact_urls_and_keep_one_record_per_line() {
    let directory = tempfile::tempdir().expect("temporary log directory");
    let path = directory.path().join("wrapper.log");
    let mut log = ShimLog::new(Some(path.clone()), "request-1".to_owned());
    log.line("failed https://user:password@example.com/private?token=secret\nforged record");
    drop(log);
    let text = std::fs::read_to_string(path).expect("wrapper log");
    assert!(text.contains("example.com"));
    assert_eq!(text.lines().count(), 1);
    for secret in ["user:", "password", "/private", "token=", "secret"] {
        assert!(!text.contains(secret), "{text}");
    }
}
