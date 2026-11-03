use super::*;

#[test]
fn wininet_decide_raise_only_lifts_values_below_target() {
    assert_eq!(wininet::decide_raise(None), Some(16));
    assert_eq!(wininet::decide_raise(Some(2)), Some(16));
    assert_eq!(wininet::decide_raise(Some(16)), None);
    assert_eq!(wininet::decide_raise(Some(32)), None);
}

#[test]
fn wininet_ensure_then_restore_puts_an_absent_value_back_to_absent() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake::default();

    let lines = run_ensure(&mut store, &m);
    assert_eq!(store.value, Some(16));
    assert_eq!(fs::read_to_string(&m).unwrap_or_default(), "absent");
    assert!(lines[0].starts_with("raised WinINet connections per server from default 2 to 16"));

    let lines = run_restore(Some(&mut store), &m);
    assert_eq!(store.value, None);
    assert_eq!(lines, ["removed WinINet connections per server override"]);
    assert!(!m.exists());
}

#[test]
fn wininet_restore_puts_a_previous_value_back_and_keeps_the_first_marker() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake { value: Some(4) };
    run_ensure(&mut store, &m);
    store.value = Some(8);
    run_ensure(&mut store, &m);
    assert_eq!(fs::read_to_string(&m).unwrap_or_default(), "4");

    let lines = run_restore(Some(&mut store), &m);
    assert_eq!(store.value, Some(4));
    assert_eq!(lines, ["restored WinINet connections per server to 4"]);
}

#[test]
fn wininet_leaves_a_user_change_alone_on_restore() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake { value: Some(4) };
    run_ensure(&mut store, &m);
    store.value = Some(24);
    assert_eq!(
        run_restore(Some(&mut store), &m),
        <[std::string::String; 0]>::default()
    );
    assert_eq!(store.value, Some(24));
    assert!(!m.exists());
}

#[test]
fn wininet_ensure_is_a_no_op_at_or_above_target() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    let mut store = Fake { value: Some(16) };
    assert_eq!(
        run_ensure(&mut store, &m),
        <[std::string::String; 0]>::default()
    );
    assert!(!m.exists());
}

#[test]
fn wininet_restore_without_a_key_drops_the_marker() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let m = marker(&dir);
    fs::create_dir_all(dir.path().join("state")).unwrap_or_else(|e| panic!("{e}"));
    fs::write(&m, "4").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(run_restore(None, &m), <[std::string::String; 0]>::default());
    assert!(!m.exists());
}
