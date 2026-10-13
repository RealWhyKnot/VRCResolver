use std::cell::RefCell;
use std::time::Duration;

use serde_json::json;
use vrcr_shared::hosts::{MARKER_IP, contains_bypass_entry, line_is_bypass_entry};
use vrcr_shared::release::{Version, is_prerelease, pick_highest_entry};
use vrcr_shared::stop::{self, CTRL_C_ARG, GRACE, KILL_WAIT, ProcessOps};
use vrcr_shared::trust_gateway::GATEWAY_HOST;
use vrcr_shared::win::ProcessInfo;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap_or_else(|| panic!("{text}"))
}

#[test]
fn versions_parse_like_system_version() {
    assert_eq!(v("1.2").to_string(), "1.2");
    assert_eq!(v("2026.10.14.0").to_string(), "2026.10.14.0");
    for bad in [
        "1",
        "1.2.3.4.5",
        "1..2",
        "1.-2",
        "a.b",
        "1.2.3.4-x",
        "1.99999999999",
    ] {
        assert_eq!(Version::parse(bad), None, "{bad}");
    }
    assert!(v("1.2") < v("1.2.0"));
    assert!(v("1.2.0") < v("1.2.0.1"));
    assert!(v("2026.9.30.9") < v("2026.10.1.0"));
    assert_eq!(Version::ZERO.to_string(), "0.0.0.0");
}

#[test]
fn tags_drop_the_v_and_any_suffix() {
    assert_eq!(Version::from_tag("v2026.10.14.0-C0A0"), Some(v("2026.10.14.0")));
    assert_eq!(Version::from_tag("V1.2.3"), Some(v("1.2.3")));
    assert_eq!(Version::from_tag("1.2.3-beta"), Some(v("1.2.3")));
    assert_eq!(Version::from_tag("nightly"), None);
}

#[test]
fn highest_entry_ignores_unparseable_tags_and_prefers_stable_on_a_tie() {
    let list = json!([
        "not an object",
        {"tag_name": "nightly"},
        {"tag_name": "v2026.10.1.0", "id": 1},
        {"tag_name": "v2026.10.3.0-ab12", "prerelease": true, "id": 2},
        {"tag_name": "v2026.10.3.0", "id": 3},
        {"tag_name": "v2026.10.2.0", "id": 4},
    ]);
    let best = pick_highest_entry(&list).unwrap_or_else(|| panic!("no entry"));
    assert_eq!(best["id"], 3);
    assert!(!is_prerelease(best));
    assert!(is_prerelease(&list[3]));
    assert!(!is_prerelease(&list[2]));
    assert!(pick_highest_entry(&json!([])).is_none());
    assert!(pick_highest_entry(&json!({"tag_name": "v1.0"})).is_none());
}

#[test]
fn bypass_entry_detection_is_token_precise() {
    assert!(line_is_bypass_entry(&format!("{MARKER_IP}\t{GATEWAY_HOST}")));
    assert!(line_is_bypass_entry(&format!(
        "  {MARKER_IP} other {GATEWAY_HOST} # note"
    )));
    assert!(!line_is_bypass_entry(&format!("# {MARKER_IP} {GATEWAY_HOST}")));
    assert!(!line_is_bypass_entry(&format!("{MARKER_IP} x{GATEWAY_HOST}")));
    assert!(contains_bypass_entry(&format!(
        "127.0.0.1 localhost\r\n{MARKER_IP} {GATEWAY_HOST}\r\n"
    )));
    assert!(!contains_bypass_entry("127.0.0.1 localhost\n"));
}

#[derive(Default)]
struct FakeOps {
    ctrl_c_ok: bool,
    exits_after_ctrl_c: bool,
    exits_after_kill: bool,
    calls: RefCell<Vec<String>>,
}

impl ProcessOps for FakeOps {
    fn ctrl_c(&self, pid: u32) -> bool {
        self.calls.borrow_mut().push(format!("ctrl_c {pid}"));
        self.ctrl_c_ok
    }

    fn wait_exit(&self, pid: u32, timeout: Duration) -> bool {
        self.calls
            .borrow_mut()
            .push(format!("wait {pid} {}", timeout.as_secs()));
        let killed = self.calls.borrow().iter().any(|c| c.starts_with("terminate"));
        if killed {
            self.exits_after_kill
        } else {
            self.exits_after_ctrl_c
        }
    }

    fn terminate(&self, pid: u32) -> bool {
        self.calls.borrow_mut().push(format!("terminate {pid}"));
        true
    }
}

#[test]
fn stop_tries_ctrl_c_before_terminating() {
    let graceful = FakeOps {
        ctrl_c_ok: true,
        exits_after_ctrl_c: true,
        ..FakeOps::default()
    };
    assert!(stop::stop(&graceful, 7));
    assert_eq!(
        *graceful.calls.borrow(),
        vec!["ctrl_c 7".to_owned(), format!("wait 7 {}", GRACE.as_secs())]
    );

    let stuck = FakeOps {
        ctrl_c_ok: true,
        exits_after_kill: true,
        ..FakeOps::default()
    };
    assert!(stop::stop(&stuck, 8));
    assert_eq!(
        *stuck.calls.borrow(),
        vec![
            "ctrl_c 8".to_owned(),
            format!("wait 8 {}", GRACE.as_secs()),
            "terminate 8".to_owned(),
            format!("wait 8 {}", KILL_WAIT.as_secs()),
        ]
    );

    let unreachable = FakeOps::default();
    assert!(!stop::stop(&unreachable, 9));
    assert_eq!(unreachable.calls.borrow()[1], "terminate 9");
}

#[test]
fn process_matching_uses_exe_stems_and_skips_ourselves() {
    let procs = [
        ProcessInfo {
            pid: 1,
            parent_pid: 0,
            exe: "vrcresolver.exe".to_owned(),
        },
        ProcessInfo {
            pid: 2,
            parent_pid: 0,
            exe: "WKVRCPROXY.EXE".to_owned(),
        },
        ProcessInfo {
            pid: 3,
            parent_pid: 0,
            exe: "vrcresolver.Updater.exe".to_owned(),
        },
        ProcessInfo {
            pid: 4,
            parent_pid: 0,
            exe: "vrcresolver.exe".to_owned(),
        },
    ];
    assert_eq!(
        stop::pids_named(&procs, &["vrcresolver", "WKVRCProxy"], 4),
        vec![1, 2]
    );
    assert!(stop::pids_named(&procs, &["other"], 0).is_empty());
}

#[test]
fn helper_mode_only_claims_its_own_argument() {
    assert_eq!(stop::helper_main(&[]), None);
    assert_eq!(stop::helper_main(&["--quit".to_owned()]), None);
    assert_eq!(stop::helper_main(&[CTRL_C_ARG.to_owned()]), Some(1));
    assert_eq!(
        stop::helper_main(&[CTRL_C_ARG.to_owned(), "x".to_owned()]),
        Some(1)
    );
}

#[cfg(windows)]
#[test]
fn real_process_helpers_terminate_and_wait() {
    use vrcr_shared::win;
    let mut child = std::process::Command::new("ping")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    let pid = child.id();
    let image = win::process_image_path(pid).unwrap_or_default();
    assert!(
        image.to_string_lossy().to_ascii_lowercase().ends_with("ping.exe"),
        "{image:?}"
    );
    assert!(!win::wait_for_exit(pid, Duration::from_millis(100)));
    assert!(win::terminate_process(pid));
    assert!(win::wait_for_exit(pid, Duration::from_secs(5)));
    let _ = child.wait();
    assert!(win::wait_for_exit(u32::MAX - 3, Duration::from_millis(10)));
    let free = win::free_disk_bytes(&std::env::temp_dir()).unwrap_or(0);
    assert!(free > 0);
}
