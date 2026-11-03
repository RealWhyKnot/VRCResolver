use super::*;

#[test]
fn session_store_writes_history_and_events() {
    let dir = tmp();
    let store = SessionStore::new(dir.path(), t0(), fixed_now);
    assert!(
        store
            .session_path()
            .file_name()
            .is_some_and(|n| n.to_string_lossy() == "session-20251009-085320.jsonl")
    );
    store.start();
    store.record_command(" status ");
    store.record_command("status");
    store.record_command("help");
    store.record_output("Warn", "careful");
    store.stop();
    store.stop();

    assert_eq!(store.recent_commands(), vec!["status", "help"]);
    assert_eq!(store.load_history(), vec!["status", "status", "help"]);
    let events = read(store.session_path());
    let lines: Vec<serde_json::Value> = events
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}")))
        .collect();
    let kinds: Vec<&str> = lines.iter().filter_map(|v| v["type"].as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "session_start",
            "command",
            "command",
            "command",
            "output:warn",
            "session_end"
        ]
    );
    assert_eq!(lines[1]["command"], "status");
    assert!(lines[1]["text"].is_null());
    assert_eq!(lines[0]["time_utc"], "2025-10-09T08:53:20.0000000Z");
}

#[test]
fn session_history_ignores_an_oversized_file_and_keeps_the_tail() {
    let dir = tmp();
    let store = SessionStore::new(dir.path(), t0(), fixed_now);
    let many: String = (0..250).map(|i| format!("cmd{i}\n\n")).collect();
    fs::write(store.history_path(), many).unwrap_or_else(|e| panic!("{e}"));
    let history = store.load_history();
    assert_eq!(history.len(), 200);
    assert_eq!(history[0], "cmd50");
    assert_eq!(history[199], "cmd249");

    fs::write(store.history_path(), "x".repeat(300 * 1024)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(store.load_history(), <[std::string::String; 0]>::default());
}
