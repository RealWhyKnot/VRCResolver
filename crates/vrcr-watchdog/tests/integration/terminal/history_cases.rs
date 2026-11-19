use super::*;

#[test]
fn history_store_persists_the_input_history() {
    let dir = tmp();
    let store = HistoryStore::new(dir.path());
    store.record_command(" status ");
    store.record_command("status");
    store.record_command("help");
    store.record_command(" ");
    assert_eq!(
        HistoryStore::new(dir.path()).load_history(),
        vec!["status", "status", "help"]
    );
}

#[test]
fn input_history_ignores_an_oversized_file_and_keeps_the_tail() {
    let dir = tmp();
    let store = HistoryStore::new(dir.path());
    let many: String = (0..250).map(|i| format!("cmd{i}\n\n")).collect();
    fs::write(store.history_path(), many).unwrap_or_else(|e| panic!("{e}"));
    let history = store.load_history();
    assert_eq!(history.len(), 200);
    assert_eq!(history[0], "cmd50");
    assert_eq!(history[199], "cmd249");

    fs::write(store.history_path(), "x".repeat(300 * 1024)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(store.load_history(), <[std::string::String; 0]>::default());
}
