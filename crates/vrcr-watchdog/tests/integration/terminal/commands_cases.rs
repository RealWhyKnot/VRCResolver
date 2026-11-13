use super::*;

#[test]
fn command_line_splits_verb_and_arguments() {
    let c = CommandLine::parse("  /Settings  set hq on ");
    assert_eq!(c.verb, "settings");
    assert_eq!(c.args, "set hq on");
    let c = CommandLine::parse("help");
    assert_eq!(c.verb, "help");
    assert_eq!(c.args, "");
}

#[test]
fn table_falls_back_to_key_value_when_narrow() {
    let rows = vec![(
        "status-line".to_owned(),
        "on".to_owned(),
        "Show the live status line.".to_owned(),
    )];
    let wide = blocks::table("settings", &rows, 100, &ASCII);
    assert!(wide[1].plain.contains("setting") && wide[1].plain.contains("about"));
    assert_eq!(wide[2].runs[2].color, Color::Green);
    let narrow = blocks::table("settings", &rows, 30, &ASCII);
    assert!(narrow.iter().any(|f| f.plain.contains("on - Show")));
}

#[test]
fn prompt_only_mode_shows_ghost_text_and_cursor() {
    let mut l = line("he", 80);
    l.status_line = false;
    l.ghost = "lp ";
    l.cursor = Some(1);
    let frame = status::format(&idle(), &no_bandwidth(), t0(), &l);
    assert_eq!(frame.plain, format!("{PROMPT}help "));
    assert_eq!(frame.runs[1].color, Color::DarkGray);
    assert_eq!(frame.cursor, Some(PROMPT.len() + 1));
    assert_eq!(status::normalize_width(0), 119);
    assert_eq!(status::normalize_width(500), 180);
}

#[test]
fn registry_completes_commands_with_and_without_a_slash() {
    let r = Registry::with_defaults();
    assert_eq!(r.complete("/he").replacement, "/help ");
    assert_eq!(r.complete("cl").replacement, "clear ");
    assert_eq!(r.complete("exi").replacement, "quit ");
    let s = r.complete("s");
    assert_eq!(s.replacement, "");
    let names: Vec<&str> = s.suggestions.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(names, vec!["settings", "status"]);
    assert_eq!(r.complete("").suggestions.len(), r.all().len());
    assert_eq!(
        r.complete("nope ").suggestions,
        <[vrcr_watchdog::terminal::blocks::Item; 0]>::default()
    );
}

#[test]
fn registry_suggests_ghost_suffixes() {
    let r = Registry::with_defaults();
    assert_eq!(r.suggest("/hi"), "story ");
    assert_eq!(r.suggest("HE"), "lp ");
    assert_eq!(r.suggest("he"), "lp ");
    assert_eq!(r.suggest("help "), "");
    assert_eq!(r.suggest("settings set high-q"), "uality ");
    assert_eq!(r.suggest(""), "");
}

#[test]
fn registry_resolves_aliases_and_nearest_matches() {
    let r = Registry::with_defaults();
    assert_eq!(r.get("/STATS").map(|c| c.name.as_str()), Some("status"));
    assert_eq!(r.get("?").map(|c| c.name.as_str()), Some("help"));
    assert!(r.get("").is_none());
    let near: Vec<String> = r.nearest("stauts", 3).into_iter().map(|i| i.text).collect();
    assert_eq!(near[0], "status");
    assert_eq!(
        r.nearest("zzzzzzzzzz", 3),
        <[vrcr_watchdog::terminal::blocks::Item; 0]>::default()
    );
    assert_eq!(
        r.nearest("", 3),
        <[vrcr_watchdog::terminal::blocks::Item; 0]>::default()
    );
    assert_eq!(edit_distance("kitten", "sitting"), 3);
    assert_eq!(edit_distance("", "abc"), 3);
}

#[test]
fn settings_completion_walks_actions_names_and_values() {
    let r = Registry::with_defaults();
    let first = r.complete("settings ");
    let texts: Vec<&str> = first.suggestions.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(&texts[..4], &["get", "set", "reset", "help"]);
    assert_eq!(texts.len(), 4 + 7);
    assert_eq!(r.complete("settings ge").replacement, "settings get ");
    assert_eq!(
        r.complete("/settings set hi").replacement,
        "/settings set high-quality "
    );
    assert_eq!(
        r.complete("settings reset al").replacement,
        "settings reset all "
    );
    assert_eq!(
        r.complete("settings set high-quality of").replacement,
        "settings set high-quality off"
    );
    assert_eq!(
        r.complete("settings set high-quality ").suggestions.len(),
        2
    );
    assert_eq!(r.complete("settings animations o").suggestions.len(), 2);
    assert_eq!(
        r.complete("settings set bogus o").suggestions,
        <[vrcr_watchdog::terminal::blocks::Item; 0]>::default()
    );
    assert_eq!(r.complete("help st").replacement, "help status");
}

#[test]
fn typing_help_echoes_and_lists_every_command() {
    let h = harness();
    h.run("/help");
    let out = h.out.text();
    assert!(out.contains("vrcr> /help\n"));
    assert!(
        out.lines()
            .any(|line| line == "commands  tab completes a unique match")
    );
    assert!(out.contains("/settings"));
    assert!(out.contains("/stats, /activity, /dashboard"));
    assert!(out.contains("  tab completes as you type; help <command> explains one."));
    assert_eq!(h.session.recent_commands(), vec!["/help"]);
}

#[test]
fn help_for_one_command_and_unknown_commands() {
    let h = harness();
    h.run("help settings");
    let out = h.out.text();
    assert!(out.lines().any(|line| line == "help"));
    assert!(out.contains("config"));
    assert!(out.contains("tab completes them"));
    h.run("stauts");
    let out = h.out.text();
    assert!(out.contains("  unknown command: stauts"));
    assert!(out.lines().any(|line| line == "matches"));
    h.run("zzzzzzzzzz");
    assert!(h.out.text().contains("type help for the full list."));
}

#[test]
fn settings_commands_change_and_reset_the_store() {
    let h = harness();
    h.run("settings set high-quality on");
    assert!(h.settings.snapshot().playback.high_quality);
    assert!(h.out.text().contains("settings: high-quality = on"));
    h.run("settings update-check off");
    assert!(
        h.out
            .text()
            .contains("settings: update-check = off (takes effect next launch)")
    );
    h.run("settings set hq maybe");
    assert!(h.out.text().contains("invalid value for hq:"));
    h.run("settings reset all");
    assert!(!h.settings.snapshot().playback.high_quality);
    assert!(h.settings.snapshot().maintenance.update_check);
    h.run("settings get bogus");
    assert!(
        h.out
            .text()
            .contains("unknown setting: bogus (use settings to list names)")
    );
    h.run("settings get");
    assert!(h.out.text().contains("usage: settings get <name>"));
    h.run("settings set x");
    assert!(h.out.text().contains("usage: settings set <name> <value>"));
    h.run("settings reset");
    assert!(h.out.text().contains("usage: settings reset <name|all>"));
    h.run("settings animations");
    assert!(h.out.text().lines().any(|line| line == "setting"));
    h.run("settings");
    assert!(h.out.text().contains("secure-local-video"));
    h.run("settings help");
    assert!(h.out.text().contains("settings status-line off"));
}

#[test]
fn quit_and_ctrl_d_request_shutdown() {
    let h = harness();
    h.typed("x");
    h.core.handle_key(Key::CtrlD);
    assert_eq!(h.shutdowns.load(Ordering::SeqCst), 0);
    h.core.handle_key(Key::CtrlU);
    h.core.handle_key(Key::CtrlD);
    assert_eq!(h.shutdowns.load(Ordering::SeqCst), 1);
    h.run("exit");
    assert_eq!(h.shutdowns.load(Ordering::SeqCst), 2);
    assert!(h.out.text().contains("quit requested; shutting down."));
}

#[test]
fn tab_completes_and_right_accepts_the_ghost() {
    let h = harness();
    h.typed("se");
    assert_eq!(h.core.ghost(), "ttings ");
    h.core.handle_key(Key::Right);
    assert_eq!(h.core.input_text(), "settings ");
    h.core.handle_key(Key::Escape);
    h.typed("s");
    h.core.handle_key(Key::Tab);
    assert_eq!(h.core.input_text(), "s");
    assert!(h.out.text().contains("vrcr> s\nmatches"));
    h.core.handle_key(Key::Left);
    assert_eq!(h.core.ghost(), "");
    h.core.handle_key(Key::End);
    h.typed("t");
    h.core.handle_key(Key::Tab);
    assert_eq!(h.core.input_text(), "status ");
}

#[test]
fn up_arrow_recalls_submitted_commands() {
    let h = harness();
    h.run("history");
    assert!(h.out.text().contains(" 1  history"));
    h.core.handle_key(Key::Up);
    assert_eq!(h.core.input_text(), "history");
    h.core.handle_key(Key::Down);
    assert_eq!(h.core.input_text(), "");
    h.core.handle_key(Key::Enter);
    assert_eq!(h.session.recent_commands(), vec!["history"]);
}

#[test]
fn ctrl_c_cancels_input_with_a_warning() {
    let h = harness();
    h.typed("abc");
    h.core.handle_key(Key::CtrlC);
    assert_eq!(h.core.input_text(), "");
    assert!(h.out.text().contains("input cancelled."));
}

#[test]
fn key_decoder_maps_keys_ctrl_and_surrogates() {
    let ev = |vk: u16, ch: u16, ctrl: bool| KeyEvent { vk, ch, ctrl };
    let mut d = KeyDecoder::default();
    assert_eq!(d.decode(ev(0x0D, 13, false)), Key::Enter);
    assert_eq!(d.decode(ev(0x0D, 13, true)), Key::Enter);
    assert_eq!(d.decode(ev(0x55, 0x15, true)), Key::CtrlU);
    assert_eq!(d.decode(ev(0x41, 0x01, true)), Key::Ignored);
    assert_eq!(d.decode(ev(0x51, u16::from(b'@'), true)), Key::Char('@'));
    assert_eq!(d.decode(ev(0x41, u16::from(b'a'), false)), Key::Char('a'));
    assert_eq!(d.decode(ev(0, 0xD83D, false)), Key::Ignored);
    assert_eq!(d.decode(ev(0, 0xDE00, false)), Key::Char('\u{1F600}'));
    assert_eq!(d.decode(ev(0, 0xDE00, false)), Key::Ignored);
    assert_eq!(d.decode(ev(0x26, 0, false)), Key::Up);
}

#[test]
fn completion_palette_and_items_render() {
    let frames = blocks::completion_palette(&[Item::new("status", "Show status.")], 60, &ASCII);
    assert_eq!(frames[0].plain, "matches");
    assert!(frames[1].plain.starts_with("  - status"));
}
