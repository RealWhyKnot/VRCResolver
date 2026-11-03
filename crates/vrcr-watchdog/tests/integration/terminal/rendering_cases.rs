use super::*;

#[test]
fn panel_draws_a_box_and_colors_states() {
    let rows = vec![
        (
            "resolver".to_owned(),
            "online idle".to_owned(),
            "detail".to_owned(),
        ),
        ("mesh".to_owned(), "reconnecting".to_owned(), "x".to_owned()),
    ];
    let frames = blocks::panel("status", &rows, 80, &ASCII);
    assert!(frames[0].plain.starts_with("+ status -"));
    assert!(frames[0].plain.ends_with('+'));
    assert_eq!(frames[0].width(), 80);
    assert!(frames.iter().all(|f| f.width() == 80));
    assert_eq!(frames[1].runs[3].color, Color::Green);
    assert_eq!(frames[2].runs[3].color, Color::Yellow);
    let narrow = blocks::panel("status", &rows, 40, &ASCII);
    assert!(narrow[1].plain.contains("online idle  detail"));
}

#[test]
fn status_line_uses_full_runs_when_wide() {
    let active = Activity {
        relay_bytes: 2048,
        last_relay: Some(t0()),
        ..Activity::default()
    };
    let bw = Bandwidth {
        current: 1024,
        peak: 1024,
        history: vec![0, 512, 1024],
    };
    let frame = status::format(&active, &bw, t0(), &line("he", 140));
    assert!(
        frame
            .plain
            .starts_with("VRChat | serving 1.0 KB/s now 2.0 KB served")
    );
    assert!(frame.plain.ends_with(&format!("online  {PROMPT}he")));
    assert_eq!(frame.cursor, Some(frame.width()));
    assert!(frame.plain.contains("_=#"));
}

#[test]
fn status_line_degrades_to_a_fitted_plain_line() {
    let frame = status::format(
        &idle(),
        &no_bandwidth(),
        t0(),
        &line("settings set high-quality", 40),
    );
    assert!(frame.width() <= 40);
    assert!(frame.plain.contains(PROMPT));
    assert!(frame.plain.ends_with("high-quality"));
    assert_eq!(frame.runs.len(), 1);
    assert_eq!(frame.cursor, Some(frame.width()));
}

#[test]
fn frame_gate_reuses_frames_and_ignores_the_spinner_when_still() {
    let mut gate = FrameGate::default();
    assert_eq!(gate.last_width(), None);
    let mut l = line("", 120);
    let a = gate.frame(&idle(), &no_bandwidth(), t0(), &l);
    l.spinner = 7;
    let b = gate.frame(&idle(), &no_bandwidth(), t0() + Duration::from_secs(1), &l);
    assert_eq!(a, b);
    assert_eq!(gate.last_width(), Some(120));
    l.input = "x";
    let c = gate.frame(&idle(), &no_bandwidth(), t0(), &l);
    assert_ne!(a, c);
}

#[test]
fn overlay_line_only_writes_changes_and_clears_its_width() {
    let mut out = Vec::new();
    let mut ov = OverlayLine::default();
    let frame = Frame::plain("abc", Color::Gray);
    assert!(
        ov.render_if_changed(&mut out, &frame, false)
            .unwrap_or_else(|e| panic!("{e}"))
    );
    assert!(
        !ov.render_if_changed(&mut out, &frame, false)
            .unwrap_or_else(|e| panic!("{e}"))
    );
    ov.clear(&mut out).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(String::from_utf8_lossy(&out), "abc\r   \r");
    assert!(!ov.visible());

    let mut out = Vec::new();
    park_caret(&mut out, 0).unwrap_or_else(|e| panic!("{e}"));
    park_caret(&mut out, 12).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(String::from_utf8_lossy(&out), "\r\r\x1b[12C");

    let mut out = Vec::new();
    write_runs(&mut out, &[Run::new("a", Color::Green)], true).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(String::from_utf8_lossy(&out), "\x1b[92ma\x1b[0m");
}

#[test]
fn status_tools_diagnostics_and_update_render() {
    let h = harness_with(Some(100));
    h.stats.record_relay_bytes(false, 4096, t0());
    h.run("status");
    let out = h.out.text();
    assert!(out.contains("+ status "));
    assert!(out.contains("serving VRChat"));
    assert!(out.contains("4.0 KB served"));
    h.run("tools");
    assert!(h.out.text().contains("resolver mesh"));
    h.run("diag");
    let out = h.out.text();
    assert!(out.contains("* diagnostics"));
    assert!(out.contains("this session"));
    h.run("permissions");
    assert!(h.out.text().contains("shell execution is not exposed."));
    h.run("update");
    assert!(
        h.out
            .text()
            .contains("[terminal][err] updater not found in install folder")
    );
}

#[test]
fn render_writes_the_status_line_through_the_overlay() {
    let h = harness();
    let guard = h.core.renderer().attach();
    h.typed("he");
    let out = h.out.text();
    assert!(out.contains("VRChat waiting 0 B served  resolver idle online  vrcr> help "));
    drop(guard);
    let before = h.out.text().len();
    h.core.renderer().render_overlay();
    assert_eq!(h.out.text().len(), before);
}

#[test]
fn status_rows_describe_idle_and_disconnected_states() {
    let rows = status_rows(
        &idle(),
        &no_bandwidth(),
        false,
        &AppSettings::default(),
        t0(),
        &ASCII,
    );
    let find = |n: &str| rows.iter().find(|r| r.0 == n).cloned().unwrap_or_default();
    assert_eq!(find("local video").1, "waiting");
    assert_eq!(find("local video").2, "0 B served total");
    assert_eq!(find("resolver").1, "reconnecting");
    assert_eq!(find("resolver").2, "no media pulled yet");
    assert_eq!(find("peak speed").1, "none yet");
    assert_eq!(find("resolves").2, "0 local video, 0 cache hits");
    assert_eq!(rows.len(), 8);
}
