use super::*;

#[test]
fn command_line_splits_verb_and_arguments() {
    let c = CommandLine::parse("  /Update  beta ");
    assert_eq!(c.verb, "update");
    assert_eq!(c.args, "beta");
    let c = CommandLine::parse("help");
    assert_eq!(c.verb, "help");
    assert_eq!(c.args, "");
}

#[test]
fn registry_completes_commands_with_and_without_a_slash() {
    let r = Registry::with_defaults();
    for (prefix, command) in [
        ("he", "help"),
        ("st", "status"),
        ("re", "repair"),
        ("up", "update"),
        ("cl", "clear"),
    ] {
        assert_eq!(r.complete(prefix).replacement, format!("{command} "));
        assert_eq!(
            r.complete(&format!("/{prefix}")).replacement,
            format!("/{command} ")
        );
        assert_eq!(
            r.complete(&format!("help {prefix}")).replacement,
            format!("help {command}")
        );
    }
    assert_eq!(r.complete("").suggestions.len(), r.all().len());
    assert_eq!(
        r.complete("nope ").suggestions,
        <[vrcr_watchdog::terminal::blocks::Item; 0]>::default()
    );
}

#[test]
fn registry_suggests_ghost_suffixes() {
    let r = Registry::with_defaults();
    assert_eq!(r.suggest("/re"), "pair ");
    assert_eq!(r.suggest("HE"), "lp ");
    assert_eq!(r.suggest("he"), "lp ");
    assert_eq!(r.suggest("help "), "");
    assert_eq!(r.suggest("update be"), "ta");
    assert_eq!(r.suggest(""), "");
}

#[test]
fn registry_suggests_nearest_commands() {
    let r = Registry::with_defaults();
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
fn typing_help_echoes_and_lists_every_command() {
    let h = harness();
    h.run("help");
    let out = h.out.text();
    assert!(out.contains("vrcr> help\n"));
    assert!(out.lines().any(|line| line == "commands"));
    let names: Vec<_> = out
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .strip_prefix("- ")?
                .split_whitespace()
                .next()
        })
        .collect();
    assert_eq!(names, ["help", "status", "repair", "update", "clear"]);
    assert!(out.contains("Ask Windows again for permission to play videos in public worlds."));
    assert!(out.contains("Press Tab to complete a command or type help <command> for details."));
    assert!(out.contains("To uninstall, run vrcresolver.Uninstaller.exe from this folder."));
}

#[test]
fn help_for_one_command_and_unknown_commands() {
    let h = harness();
    h.run("help update");
    let out = h.out.text();
    assert!(out.contains("update beta"));
    assert!(out.contains("Switch to beta releases and check now."));
    assert!(out.contains("update stable"));
    assert!(out.contains("Switch to stable releases and check now."));
    h.run("help repair");
    assert!(
        h.out
            .text()
            .contains("Ask Windows again for permission to play videos in public worlds.")
    );
    h.run("stauts");
    let out = h.out.text();
    assert!(out.contains("  unknown command: stauts"));
    assert!(out.lines().any(|line| line == "matches"));
    h.run("zzzzzzzzzz");
    assert!(h.out.text().contains("type help for the full list."));
}

#[test]
fn repair_starts_without_arguments() {
    let h = harness();
    h.run("repair");
    assert_eq!(h.repairs.load(Ordering::SeqCst), 1);
}

#[test]
fn tab_completes_and_right_accepts_the_ghost() {
    let h = harness();
    h.typed("up");
    assert_eq!(h.core.ghost(), "date ");
    h.core.handle_key(Key::Right);
    assert_eq!(h.core.input_text(), "update ");
    h.core.handle_key(Key::Tab);
    assert_eq!(h.core.input_text(), "update ");
    assert!(h.out.text().contains("vrcr> update \nmatches"));
    h.typed("b");
    h.core.handle_key(Key::Tab);
    assert_eq!(h.core.input_text(), "update beta");
}

#[test]
fn up_arrow_recalls_submitted_commands() {
    let h = harness();
    h.run("status");
    h.core.handle_key(Key::Up);
    assert_eq!(h.core.input_text(), "status");
    h.core.handle_key(Key::Down);
    assert_eq!(h.core.input_text(), "");
    h.core.handle_key(Key::Enter);
    assert_eq!(
        HistoryStore::new(&h._dir.path().join("terminal")).load_history(),
        ["status"]
    );
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

#[test]
fn removed_commands_have_no_completions() {
    let registry = Registry::with_defaults();
    for removed in [
        "settings",
        "logs",
        "quit",
        "exit",
        "history",
        "permissions",
        "tools",
        "diagnostics",
    ] {
        assert!(registry.get(removed).is_none(), "{removed}");
        assert_eq!(registry.complete(removed), Default::default(), "{removed}");
    }
}

#[test]
fn update_completes_beta_and_stable() {
    let registry = Registry::with_defaults();
    assert_eq!(registry.complete("update b").replacement, "update beta");
    assert_eq!(registry.complete("update ST").replacement, "update stable");
    let choices = registry.complete("update ").suggestions;
    assert_eq!(
        choices
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["beta", "stable"]
    );
    assert_eq!(registry.complete("update stable extra"), Default::default());
}

#[test]
fn update_switches_channel_and_checks_the_saved_choice() {
    let h = harness();
    let load =
        || Store::new(h._dir.path().join("settings.json"), Box::new(|_| {})).include_prereleases();
    h.run("update beta");
    assert!(h.settings.include_prereleases());
    assert!(load());
    assert!(h.out.text().ends_with("  Checking for beta updates.\n"));
    h.run("update");
    assert!(h.out.text().ends_with("  Checking for beta updates.\n"));
    h.run("update stable");
    assert!(!h.settings.include_prereleases());
    assert!(!load());
    assert!(h.out.text().ends_with("  Checking for stable updates.\n"));
    h.run("update");
    assert!(h.out.text().ends_with("  Checking for stable updates.\n"));
    assert_eq!(h.update_checks.load(Ordering::SeqCst), 4);
    h.run("update nightly");
    assert!(
        h.out
            .text()
            .ends_with("  Type update, update beta or update stable.\n")
    );
    assert!(!load());
    assert_eq!(h.update_checks.load(Ordering::SeqCst), 4);
}

fn set_check_start(h: &Harness, start: CheckStart) {
    *h.update_start.lock().unwrap_or_else(|e| e.into_inner()) = start;
}

#[test]
fn update_reports_a_busy_check_an_open_window_or_a_running_install() {
    let h = harness();
    for (start, reply) in [
        (CheckStart::Checking, "  Already checking for updates.\n"),
        (
            CheckStart::Prompting,
            "  The update window is already open.\n",
        ),
        (CheckStart::Installing, "  An update is already running.\n"),
    ] {
        set_check_start(&h, start);
        h.run("update");
        assert!(h.out.text().ends_with(reply), "{start:?}");
        h.run("update stable");
        assert!(h.out.text().ends_with(reply), "{start:?}");
    }
    assert_eq!(h.update_checks.load(Ordering::SeqCst), 6);
}

#[test]
fn switching_channel_while_busy_says_the_choice_was_saved() {
    let h = harness();
    set_check_start(&h, CheckStart::Checking);
    h.run("update beta");
    assert!(h.settings.include_prereleases());
    assert!(h.out.text().ends_with(
        "  Switched to beta releases. Type update again when the current check finishes.\n"
    ));
    set_check_start(&h, CheckStart::Prompting);
    h.run("update stable");
    assert!(!h.settings.include_prereleases());
    assert!(h.out.text().ends_with(
        "  Switched to stable releases. Close the update window, then type update again.\n"
    ));
    set_check_start(&h, CheckStart::Installing);
    h.run("update beta");
    assert!(
        h.out
            .text()
            .ends_with("  Switched to beta releases. An update is already running.\n")
    );
}
