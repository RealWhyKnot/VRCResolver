use super::*;

#[test]
fn identity_first_call_generates_and_persists() {
    let f = id_file();
    assert!(!f.path.exists());
    let id = load_or_create(&f.path);
    assert!(is_simple_guid(&id), "{id}");
    assert_eq!(read_trimmed(&f.path), id);
}

#[test]
fn identity_later_calls_return_the_same_id() {
    let f = id_file();
    let first = load_or_create(&f.path);
    assert_eq!(load_or_create(&f.path), first);
    assert_eq!(load_or_create(&f.path), first);
}

#[test]
fn identity_corrupt_file_is_regenerated() {
    let f = id_file();
    fs::write(&f.path, "not-a-guid").unwrap_or_else(|e| panic!("{e}"));
    let id = load_or_create(&f.path);
    assert!(is_simple_guid(&id));
    assert_eq!(read_trimmed(&f.path), id);
    assert_eq!(load_or_create(&f.path), id);
}

#[test]
fn identity_surrounding_whitespace_is_accepted() {
    let f = id_file();
    let g = uuid::Uuid::new_v4().simple().to_string();
    fs::write(&f.path, format!("  {g}\r\n")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(load_or_create(&f.path), g);
}

#[test]
fn identity_dashed_guid_is_rejected() {
    let f = id_file();
    let g = uuid::Uuid::new_v4();
    fs::write(&f.path, g.hyphenated().to_string()).unwrap_or_else(|e| panic!("{e}"));
    let id = load_or_create(&f.path);
    assert_ne!(id, g.simple().to_string());
    assert!(is_simple_guid(&id));
}

#[test]
fn identity_write_leaves_no_temp_file() {
    let f = id_file();
    load_or_create(&f.path);
    assert!(f.path.exists());
    assert!(!identity::sibling(&f.path, ".new").exists());
}

#[test]
fn identity_missing_parent_dirs_are_created() {
    let f = id_file();
    let deep = f
        .dir
        .join("subdir")
        .join("nested")
        .join(identity::FILE_NAME);
    let id = load_or_create(&deep);
    assert!(is_simple_guid(&id));
    assert!(deep.exists());
}

#[test]
fn identity_oversized_file_is_replaced() {
    let f = id_file();
    let cap = usize::try_from(identity::MAX_FILE_BYTES).unwrap_or(256);
    fs::write(&f.path, vec![0u8; cap + 1]).unwrap_or_else(|e| panic!("{e}"));
    let id = load_or_create(&f.path);
    assert!(is_simple_guid(&id));
    assert_eq!(read_trimmed(&f.path), id);
    assert_eq!(fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0), 32);
}

#[test]
fn identity_parse_lowercases_and_checks_length() {
    assert_eq!(
        parse_simple_guid("ABCDEF0123456789ABCDEF0123456789").as_deref(),
        Some("abcdef0123456789abcdef0123456789")
    );
    assert_eq!(parse_simple_guid("abc"), None);
    assert_eq!(parse_simple_guid("zbcdef0123456789abcdef0123456789"), None);
}

#[cfg(windows)]
#[test]
fn identity_survives_a_brief_lock() {
    let f = id_file();
    let stored = load_or_create(&f.path);
    let mut lock = Some(lock_exclusively(&f.path));
    let mut waits = Vec::new();
    let id = load_or_create_with(&f.path, &mut |d| {
        waits.push(d);
        drop(lock.take());
    });
    assert_eq!(waits, vec![Duration::from_millis(50)]);
    assert_eq!(id, stored);
    assert_eq!(read_trimmed(&f.path), stored);
}

#[cfg(windows)]
#[test]
fn identity_does_not_overwrite_a_locked_file() {
    let f = id_file();
    let stored = load_or_create(&f.path);
    let before: Vec<_> = list(&f.dir);
    let id = {
        let _lock = lock_exclusively(&f.path);
        load_or_create(&f.path)
    };
    assert!(is_simple_guid(&id));
    assert_ne!(id, stored);
    assert_eq!(read_trimmed(&f.path), stored);
    assert_eq!(list(&f.dir), before);
}

#[cfg(windows)]
#[test]
fn identity_failed_save_cleans_its_temp_file() {
    let f = id_file();
    fs::write(&f.path, "corrupt").unwrap_or_else(|e| panic!("{e}"));
    let id = {
        let _lock = lock_exclusively(&f.path);
        load_or_create(&f.path)
    };
    assert!(is_simple_guid(&id));
    assert!(!identity::sibling(&f.path, ".new").exists());
}
