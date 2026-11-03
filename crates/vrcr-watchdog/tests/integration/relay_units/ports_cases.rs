use super::*;

#[test]
fn port_manager_replaces_busy_previous_port() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap_or_else(|e| panic!("{e}"));
    let busy_port = busy.local_addr().unwrap_or_else(|e| panic!("{e}")).port();
    fs::write(
        dir.path().join("relay_last_port.txt"),
        busy_port.to_string(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    assert!(ports.initialize());
    assert_ne!(ports.current_port(), busy_port);
    assert_eq!(
        read(&dir, "relay_port.txt"),
        ports.current_port().to_string()
    );
    assert_eq!(
        read(&dir, "relay_last_port.txt"),
        ports.current_port().to_string()
    );
    assert!(
        out.text().contains("(previous port was busy)"),
        "{}",
        out.text()
    );
}

#[test]
fn port_manager_reuses_a_free_previous_port() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let free = {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap_or_else(|e| panic!("{e}"));
        l.local_addr().unwrap_or_else(|e| panic!("{e}")).port()
    };
    fs::write(dir.path().join("relay_last_port.txt"), format!(" {free}\n"))
        .unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    assert!(ports.initialize());
    assert_eq!(ports.current_port(), free);
    assert!(
        out.text()
            .contains(&format!("reserved local video port {free} (reused)"))
    );
}

#[test]
fn port_manager_ignores_privileged_or_garbage_previous_ports() {
    for junk in ["80", "1024", "abc", "70000", ""] {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("relay_last_port.txt"), junk).unwrap_or_else(|e| panic!("{e}"));
        let (ux, out) = capture_ux();
        let ports = RelayPortManager::new(dir.path(), ux);
        assert!(ports.initialize());
        assert!(ports.current_port() > 0);
        assert!(!out.text().contains("busy"), "{junk}: {}", out.text());
    }
}

#[test]
fn port_manager_fresh_port_rewrites_relay_files() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    assert!(ports.initialize());
    let first = ports.current_port();
    let _busy = TcpListener::bind((Ipv4Addr::LOCALHOST, first)).unwrap_or_else(|e| panic!("{e}"));
    assert!(ports.try_reserve_fresh_port("test bind failure"));
    assert_ne!(ports.current_port(), 0);
    assert_ne!(ports.current_port(), first);
    assert_eq!(
        read(&dir, "relay_port.txt"),
        ports.current_port().to_string()
    );
    assert_eq!(
        read(&dir, "relay_last_port.txt"),
        ports.current_port().to_string()
    );
    assert!(out.text().contains(&format!(
        "(previous port {first} failed: test bind failure)"
    )));
    assert!(!dir.path().join("relay_port.txt.tmp").exists());
}

#[test]
fn port_manager_scheme_file_falls_back_to_http() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, _) = capture_ux();
    let ports = RelayPortManager::new(dir.path(), ux);
    ports.write_scheme_file("HTTPS");
    assert_eq!(read(&dir, "relay_scheme.txt"), "https");
    ports.write_scheme_file("ftp");
    assert_eq!(read(&dir, "relay_scheme.txt"), "http");
    ports.delete_port_file();
    assert!(!dir.path().join("relay_scheme.txt").exists());
}

#[test]
fn bypass_hides_port_file_while_wedged_and_restores_on_new_session() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (ux, out) = capture_ux();
    let ports = Arc::new(RelayPortManager::new(dir.path(), ux.clone()));
    assert!(ports.initialize());
    ports.write_scheme_file("https");
    let port_file = dir.path().join("relay_port.txt");
    let bypass = RelayBypass::new(std::sync::Arc::clone(&ports), "https", ux);

    assert!(!bypass.engage(ports.current_port() + 1));
    assert!(port_file.exists());
    assert!(bypass.engage(ports.current_port()));
    assert!(bypass.active());
    assert!(!port_file.exists());
    assert!(bypass.engage(ports.current_port()));
    assert_eq!(out.text().matches("can no longer reach").count(), 1);

    bypass.release();
    assert!(!bypass.active());
    assert_eq!(
        read(&dir, "relay_port.txt"),
        ports.current_port().to_string()
    );
    assert_eq!(read(&dir, "relay_scheme.txt"), "https");
    assert!(out.text().contains(&format!(
        "new VRChat session; local video relay back on port {}",
        ports.current_port()
    )));
    bypass.release();
    assert_eq!(out.text().matches("new VRChat session").count(), 1);
}
