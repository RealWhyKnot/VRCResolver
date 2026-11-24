use vrcr_watchdog::maintenance::dialog_owner::{
    ConsoleFacts, ConsoleKind, OwnerChoice, WindowShown, choose_owner,
};

const STATES: [WindowShown; 4] = [
    WindowShown::Missing,
    WindowShown::Hidden,
    WindowShown::Minimized,
    WindowShown::Visible,
];

fn facts(console: WindowShown, kind: ConsoleKind, root_owner: WindowShown) -> ConsoleFacts {
    ConsoleFacts {
        console,
        kind,
        root_owner,
    }
}

#[test]
fn visible_classic_console_owns_the_dialog() {
    for root_owner in STATES {
        assert_eq!(
            choose_owner(facts(
                WindowShown::Visible,
                ConsoleKind::Classic,
                root_owner
            )),
            OwnerChoice::Console
        );
    }
}

#[test]
fn hidden_or_minimized_classic_console_leaves_the_dialog_unowned() {
    for console in [WindowShown::Hidden, WindowShown::Minimized] {
        for root_owner in STATES {
            assert_eq!(
                choose_owner(facts(console, ConsoleKind::Classic, root_owner)),
                OwnerChoice::NoOwner
            );
        }
    }
}

#[test]
fn missing_console_leaves_the_dialog_unowned() {
    for kind in [ConsoleKind::Classic, ConsoleKind::Pseudo] {
        for root_owner in STATES {
            assert_eq!(
                choose_owner(facts(WindowShown::Missing, kind, root_owner)),
                OwnerChoice::NoOwner
            );
        }
    }
}

#[test]
fn pseudo_console_hands_ownership_to_a_visible_terminal_window() {
    for console in [
        WindowShown::Hidden,
        WindowShown::Minimized,
        WindowShown::Visible,
    ] {
        assert_eq!(
            choose_owner(facts(console, ConsoleKind::Pseudo, WindowShown::Visible)),
            OwnerChoice::RootOwner
        );
    }
}

#[test]
fn pseudo_console_without_a_usable_terminal_window_leaves_the_dialog_unowned() {
    for console in [
        WindowShown::Hidden,
        WindowShown::Minimized,
        WindowShown::Visible,
    ] {
        for root_owner in [
            WindowShown::Missing,
            WindowShown::Hidden,
            WindowShown::Minimized,
        ] {
            assert_eq!(
                choose_owner(facts(console, ConsoleKind::Pseudo, root_owner)),
                OwnerChoice::NoOwner
            );
        }
    }
}

#[test]
fn every_combination_picks_only_a_visible_window() {
    for console in STATES {
        for kind in [ConsoleKind::Classic, ConsoleKind::Pseudo] {
            for root_owner in STATES {
                let observed = facts(console, kind, root_owner);
                match choose_owner(observed) {
                    OwnerChoice::Console => {
                        assert_eq!(
                            (console, kind),
                            (WindowShown::Visible, ConsoleKind::Classic)
                        );
                    }
                    OwnerChoice::RootOwner => {
                        assert_ne!(console, WindowShown::Missing);
                        assert_eq!(
                            (kind, root_owner),
                            (ConsoleKind::Pseudo, WindowShown::Visible)
                        );
                    }
                    OwnerChoice::NoOwner => {}
                }
            }
        }
    }
}
