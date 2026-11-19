use std::cell::RefCell;
use std::fs;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::{NamedTempFile, TempDir};
use vrcr_shared::console::Style;
use vrcr_shared::win::{Elevated, MachineStore};

use super::*;
use crate::clock;
use crate::relay::tls::{RelayTls, TlsHost};

struct TestCertificateStore {
    cert: Mutex<Vec<Vec<u8>>>,
    prompts: AtomicUsize,
}

impl TlsHost for TestCertificateStore {
    fn store_certs(&self, _store: MachineStore) -> io::Result<Vec<Vec<u8>>> {
        Ok(self.cert.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn store_add(&self, _store: MachineStore, der: &[u8]) -> io::Result<()> {
        self.cert
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(der.to_vec());
        Ok(())
    }

    fn store_remove(
        &self,
        _store: MachineStore,
        _remove: &dyn Fn(&[u8]) -> bool,
    ) -> io::Result<usize> {
        Ok(0)
    }

    fn protect(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    fn unprotect(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    fn elevate(&self, args: &str) -> Elevated {
        self.prompts.fetch_add(1, Ordering::Relaxed);
        let der = hex::decode(args.split_whitespace().last().unwrap_or_default())
            .unwrap_or_else(|e| panic!("{e}"));
        self.store_add(MachineStore::Root, &der)
            .unwrap_or_else(|e| panic!("{e}"));
        Elevated::Exited(0)
    }

    fn delete_legacy_binding(&self, _port: u16) -> io::Result<()> {
        Ok(())
    }
}

fn console() -> (Ux, NamedTempFile) {
    let output = NamedTempFile::new().unwrap_or_else(|e| panic!("{e}"));
    let writer = output.reopen().unwrap_or_else(|e| panic!("{e}"));
    let ux = Ux::new(Box::new(writer), Style::plain(), String::new);
    (ux, output)
}

fn certificate_store() -> TestCertificateStore {
    TestCertificateStore {
        cert: Mutex::default(),
        prompts: AtomicUsize::new(0),
    }
}

#[test]
fn an_invalid_relay_port_only_fails_repair_when_trust_is_missing() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    let store = certificate_store();
    let relay = RelayTls::new(&store, dir.path(), Ux::sink(), clock::system());

    assert_eq!(relay.repair_trust(0), Err(SetupError::Failed));
    assert_eq!(store.prompts.load(Ordering::Relaxed), 0);
    assert!(!dir.path().join(crate::relay::tls::CERT_FILE).exists());
    assert_eq!(relay.repair_trust(51234), Ok(true));
    assert_eq!(relay.repair_trust(0), Ok(false));
    store
        .cert
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
    assert_eq!(relay.repair_trust(0), Err(SetupError::Failed));
    assert_eq!(store.prompts.load(Ordering::Relaxed), 1);
}

#[test]
fn both_changes_share_one_yes_and_completed_setup_skips_the_dialog() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    let store = certificate_store();
    let ux = Ux::sink();
    let installed = Arc::new(AtomicBool::new(false));
    let check = Arc::clone(&installed);
    let hosts = HostsManager::new(
        Box::new(move || Ok(check.load(Ordering::Relaxed))),
        Box::new(move |_| {
            installed.store(true, Ordering::Relaxed);
            Elevated::Exited(0)
        }),
        ux.clone(),
    );
    let closing = CancellationToken::new();
    let shown = RefCell::new(Vec::new());
    let (tls, entry) = run(
        &hosts,
        false,
        &closing,
        &|changes| {
            shown.borrow_mut().push(changes);
            Decision::Yes
        },
        &ux,
        |approve| {
            RelayTls::new(&store, dir.path(), ux.clone(), clock::system())
                .with_permission(approve)
                .ensure_ready(51234)
        },
    );
    assert!(tls.is_some());
    assert_eq!(entry, Ok(true));
    assert_eq!(*shown.borrow(), [Changes::Both]);
    assert_eq!(store.prompts.load(Ordering::Relaxed), 1);

    let changes = run(
        &hosts,
        true,
        &closing,
        &|_| panic!("completed setup asked for permission"),
        &ux,
        |approve| {
            RelayTls::new(&store, dir.path(), ux.clone(), clock::system())
                .with_permission(approve)
                .repair_trust(51234)
        },
    );
    assert_eq!(changes, (Ok(false), Ok(false)));
    assert_eq!(store.prompts.load(Ordering::Relaxed), 1);
}

#[test]
fn no_close_and_timeout_skip_both_elevations_and_repair_can_ask_again() {
    for decision in [Decision::No, Decision::Closed, Decision::TimedOut] {
        let dir = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let store = certificate_store();
        let (ux, output) = console();
        let log = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let sink = vrcr_shared::log::Log::install(log.path(), "setup");
        ux.tee_to(sink.clone());
        let hosts = HostsManager::new(
            Box::new(|| Ok(false)),
            Box::new(|_| panic!("declined setup launched an administrator prompt")),
            ux.clone(),
        );
        let closing = CancellationToken::new();
        let shown = RefCell::new(Vec::new());
        let dialog = |changes| {
            assert!(!hosts.declined_this_session());
            shown.borrow_mut().push(changes);
            decision
        };
        let (tls, entry) = run(&hosts, false, &closing, &dialog, &ux, |approve| {
            RelayTls::new(&store, dir.path(), ux.clone(), clock::system())
                .with_permission(approve)
                .ensure_ready(51234)
        });
        assert!(tls.is_none());
        assert_eq!(entry, Err(SetupError::Declined));
        assert!(hosts.declined_this_session());
        hosts.ensure_entry();
        let text = fs::read_to_string(output.path()).unwrap_or_default();
        assert!(text.contains(
            "Permission for videos in public worlds was declined. Type repair to be asked again."
        ));
        assert!(
            text.contains(
                "Permission for secure video was declined. Type repair to be asked again."
            )
        );
        let changes = run(&hosts, true, &closing, &dialog, &ux, |approve| {
            RelayTls::new(&store, dir.path(), ux.clone(), clock::system())
                .with_permission(approve)
                .repair_trust(51234)
        });
        assert_eq!(
            changes,
            (Err(SetupError::Declined), Err(SetupError::Declined))
        );
        assert_eq!(*shown.borrow(), [Changes::Both, Changes::Both]);
        assert!(hosts.declined_this_session());
        assert_eq!(store.prompts.load(Ordering::Relaxed), 0);
        let path = sink
            .current_path()
            .unwrap_or_else(|| panic!("setup log was not created"));
        sink.close();
        let lines = fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            lines.contains("The Windows permission dialog timed out."),
            decision == Decision::TimedOut
        );
    }
}

#[test]
fn dialog_only_names_the_change_that_needs_permission() {
    let ux = Ux::sink();
    for (hosts_present, wanted) in [(true, Changes::Certificate), (false, Changes::Hosts)] {
        let dir = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let hosts = HostsManager::new(
            Box::new(move || Ok(hosts_present)),
            Box::new(|_| panic!("declined setup elevated")),
            ux.clone(),
        );
        let store = certificate_store();
        let closing = CancellationToken::new();
        let shown = RefCell::new(Vec::new());
        let _ = run(
            &hosts,
            true,
            &closing,
            &|changes| {
                shown.borrow_mut().push(changes);
                Decision::No
            },
            &ux,
            |approve| {
                RelayTls::new(&store, dir.path(), ux.clone(), clock::system())
                    .with_permission(approve)
                    .disabled(!hosts_present)
                    .repair_trust(51234)
            },
        );
        assert_eq!(*shown.borrow(), [wanted]);
    }
}

#[test]
fn certificate_only_consent_does_not_cover_a_hosts_entry_that_goes_missing() {
    let ux = Ux::sink();
    let installed = Arc::new(AtomicBool::new(true));
    let check = Arc::clone(&installed);
    let hosts = HostsManager::new(
        Box::new(move || Ok(check.load(Ordering::Relaxed))),
        Box::new(|_| panic!("hosts change was not approved")),
        ux.clone(),
    );
    let (_, entry) = run(
        &hosts,
        false,
        &CancellationToken::new(),
        &|changes| {
            assert_eq!(changes, Changes::Certificate);
            Decision::Yes
        },
        &ux,
        |approve| {
            assert!(approve());
            installed.store(false, Ordering::Relaxed);
        },
    );
    assert_eq!(entry, Err(SetupError::Declined));
}
