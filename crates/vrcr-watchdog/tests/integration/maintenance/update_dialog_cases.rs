use vrcr_watchdog::maintenance::update::Release;
use vrcr_watchdog::maintenance::update_dialog::{
    self, DialogButton, LATER_BUTTON, SKIP_VERSION_BUTTON, UPDATE_NOW_BUTTON, UpdateDecision,
    decision_for, dialog_copy, dialog_flags, format_release_notes, is_allowed_release_url,
};
use windows_sys::Win32::UI::Controls::{
    TDF_ALLOW_DIALOG_CANCELLATION, TDF_ENABLE_HYPERLINKS, TDF_EXPANDED_BY_DEFAULT,
    TDF_USE_COMMAND_LINKS, TDF_USE_COMMAND_LINKS_NO_ICON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{IDCANCEL, IDCLOSE};

fn release(tag: &str, body: Option<&str>) -> Release {
    Release {
        tag: tag.to_owned(),
        html_url: format!("https://github.com/RealWhyKnot/VRCResolver/releases/tag/{tag}"),
        body: body.map(str::to_owned),
        prerelease: false,
    }
}

#[test]
fn dialog_copy_names_both_versions_and_three_choices() {
    let copy = dialog_copy(&release("v2026.11.24.0", None), "2026.11.20.0");
    assert_eq!(copy.title, "VRCResolver Update");
    assert_eq!(copy.instruction, "VRCResolver 2026.11.24.0 is available");
    assert_eq!(
        copy.content,
        "You're running 2026.11.20.0. Installing will restart VRCResolver."
    );
    assert_eq!(
        copy.buttons,
        vec![
            DialogButton {
                id: UPDATE_NOW_BUTTON,
                text: "Update now\nDownload and install 2026.11.24.0".to_owned(),
            },
            DialogButton {
                id: LATER_BUTTON,
                text: "Not now\nAsk again next time VRCResolver starts".to_owned(),
            },
            DialogButton {
                id: SKIP_VERSION_BUTTON,
                text:
                    "Don't ask again for this version\nNotify again when a newer release comes out"
                        .to_owned(),
            },
        ]
    );
    assert_eq!(copy.default_button, UPDATE_NOW_BUTTON);
}

#[test]
fn dialog_copy_drops_the_running_version_when_unknown() {
    for current in ["", "   "] {
        assert_eq!(
            dialog_copy(&release("2026.11.24.0", None), current).content,
            "Installing will restart VRCResolver."
        );
    }
}

#[test]
fn dialog_uses_cancellable_command_links() {
    assert_eq!(
        update_dialog::FLAGS,
        TDF_USE_COMMAND_LINKS | TDF_ALLOW_DIALOG_CANCELLATION | TDF_ENABLE_HYPERLINKS
    );
    assert_eq!(update_dialog::FLAGS & TDF_USE_COMMAND_LINKS_NO_ICON, 0);
}

#[test]
fn button_ids_map_to_decisions_and_cancel_means_later() {
    let ids = [UPDATE_NOW_BUTTON, LATER_BUTTON, SKIP_VERSION_BUTTON];
    assert_eq!(
        ids.len(),
        ids.iter().collect::<std::collections::HashSet<_>>().len()
    );
    assert!(ids.iter().all(|id| *id != IDCANCEL && *id != IDCLOSE));
    assert_eq!(decision_for(UPDATE_NOW_BUTTON), UpdateDecision::UpdateNow);
    assert_eq!(decision_for(LATER_BUTTON), UpdateDecision::Later);
    assert_eq!(
        decision_for(SKIP_VERSION_BUTTON),
        UpdateDecision::SkipVersion
    );
    for other in [IDCANCEL, IDCLOSE, 0, -1, 9999] {
        assert_eq!(decision_for(other), UpdateDecision::Later);
    }
}

#[test]
fn dialog_without_notes_has_no_expander_but_keeps_the_footer_link() {
    let copy = dialog_copy(&release("v2026.11.24.0", None), "2026.11.20.0");
    assert_eq!(copy.notes, None);
    assert_eq!(
        copy.footer.as_deref(),
        Some(
            "<a href=\"https://github.com/RealWhyKnot/VRCResolver/releases/tag/v2026.11.24.0\">View full release notes on GitHub</a>"
        )
    );
    assert_eq!(dialog_flags(&copy) & TDF_EXPANDED_BY_DEFAULT, 0);
}

#[test]
fn dialog_with_notes_starts_expanded() {
    let copy = dialog_copy(
        &release("v2026.11.24.0", Some("## Fixes\n- Faster resolves")),
        "2026.11.20.0",
    );
    assert_eq!(
        copy.notes.as_deref(),
        Some("Fixes\n\u{2022} Faster resolves")
    );
    assert_ne!(dialog_flags(&copy) & TDF_EXPANDED_BY_DEFAULT, 0);
    assert_ne!(dialog_flags(&copy) & TDF_ENABLE_HYPERLINKS, 0);
}

#[test]
fn dialog_with_only_markup_notes_has_no_expander() {
    let copy = dialog_copy(
        &release("v2026.11.24.0", Some("<!-- internal -->\n![shot](a.png)")),
        "2026.11.20.0",
    );
    assert_eq!(copy.notes, None);
    assert_eq!(dialog_flags(&copy) & TDF_EXPANDED_BY_DEFAULT, 0);
}

#[test]
fn untrusted_release_url_falls_back_to_the_tag_page_in_the_footer() {
    let mut hostile = release("v2026.11.24.0", None);
    for url in [
        "https://evil.test/\"><a href=\"x",
        "https://github.com/a\"onmouseover=\"x",
        "https://github.com/someone/else/releases/tag/v2026.11.24.0",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1\"><a href=\"x",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/../../../evil",
    ] {
        hostile.html_url = url.to_owned();
        assert_eq!(
            dialog_copy(&hostile, "2026.11.20.0").footer.as_deref(),
            Some(
                "<a href=\"https://github.com/RealWhyKnot/VRCResolver/releases/tag/v2026.11.24.0\">View full release notes on GitHub</a>"
            ),
            "{url}"
        );
    }
}

#[test]
fn hostile_tag_falls_back_to_the_releases_page() {
    let mut hostile = release("v1\"><a href=\"https://evil.test\">x</a>", None);
    hostile.html_url = String::new();
    assert_eq!(
        dialog_copy(&hostile, "").footer.as_deref(),
        Some(
            "<a href=\"https://github.com/RealWhyKnot/VRCResolver/releases\">View full release notes on GitHub</a>"
        )
    );
}

#[test]
fn only_this_repos_release_pages_may_open() {
    for good in [
        "https://github.com/RealWhyKnot/VRCResolver/releases",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v2026.11.23.0-beta.12",
    ] {
        assert!(is_allowed_release_url(good), "{good}");
    }
    for bad in [
        "",
        "https://github.com/",
        "https://github.com/x",
        "https://github.com/RealWhyKnot/VRCResolver",
        "https://github.com/RealWhyKnot/VRCResolver/releases/",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/..",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1/../../../x",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1?x=1",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1#x",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1\"",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1<a>",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v1&x",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v 1",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v\u{0}1",
        "https://github.com/RealWhyKnot/VRCResolver/releases/tag/v\u{e9}",
        "https://github.com/RealWhyKnot/VRCResolverX/releases",
        "https://github.com/RealWhyKnot/VRCResolver/releasesX",
        "http://github.com/RealWhyKnot/VRCResolver/releases",
        "https://github.com.evil.test/RealWhyKnot/VRCResolver/releases",
        "https://evil.test/https://github.com/RealWhyKnot/VRCResolver/releases",
        "file:///c:/windows/system32/calc.exe",
        "calc.exe",
        " https://github.com/RealWhyKnot/VRCResolver/releases",
    ] {
        assert!(!is_allowed_release_url(bad), "{bad:?}");
    }
}

#[test]
fn release_notes_drop_comments_images_and_badges() {
    let body = "<!-- hidden\nnote -->Intro\n[![build](https://x/b.svg)](https://x/ci)\n![shot](https://x/s.png)\nDone";
    assert_eq!(format_release_notes(body).as_deref(), Some("Intro\n\nDone"));
    assert_eq!(
        format_release_notes("Kept\n<!-- never closed\nhidden").as_deref(),
        Some("Kept")
    );
}

#[test]
fn release_notes_keep_link_labels_only() {
    assert_eq!(
        format_release_notes("See [the docs](https://github.com/a/b) and [more](u).").as_deref(),
        Some("See the docs and more.")
    );
}

#[test]
fn release_notes_strip_headings_bold_and_code_but_keep_identifiers() {
    let body = "# Big\n### Small ###\n**Bold** and `code` with snake_case_name and __dunder__\nSupport C#\n## Added C#\n## Issue #12 fixed";
    assert_eq!(
        format_release_notes(body).as_deref(),
        Some(
            "Big\nSmall\nBold and code with snake_case_name and __dunder__\nSupport C#\nAdded C#\nIssue #12 fixed"
        )
    );
}

#[test]
fn release_notes_drop_quote_markers_fences_and_escapes() {
    let body = "> quoted\n>> deeper\n```rust\nlet x = 1;\n```\n\\*not bold\\* and 1\\. step";
    assert_eq!(
        format_release_notes(body).as_deref(),
        Some("quoted\ndeeper\nlet x = 1;\n*not bold* and 1. step")
    );
}

#[test]
fn release_notes_keep_generic_types_but_drop_html() {
    let body =
        "<details><summary>Full list</summary>\n\nReturns `Option<String>` now<br>\n</details>";
    assert_eq!(
        format_release_notes(body).as_deref(),
        Some("Full list\n\nReturns Option<String> now")
    );
}

#[test]
fn release_notes_never_carry_anchor_tags() {
    let body = "Go <a href=\"https://evil.test\">here</A> or <a\nhref='x'>there</a> <img src=x>";
    let notes = format_release_notes(body).unwrap_or_default();
    assert_eq!(notes, "Go here or there");
}

#[test]
fn release_notes_cannot_rebuild_an_anchor_from_pieces() {
    for body in [
        "<<a>a href=\"https://github.com/evil\">x<</a>/a>",
        "<<A HREF=x>A HREF=\"https://github.com/evil\">x</a>",
        "<**a** href=\"https://github.com/evil\">x</a>",
        "<`a` href=\"https://github.com/evil\">x</a>",
        "<\u{1}a href=\"https://github.com/evil\">x</a>",
        "\\<a href=\"https://github.com/evil\">x\\</a>",
        "< a href=\"https://github.com/evil\">x</ a>",
        "<a href=\"https://github.com/evil\"",
        "- <a href=\"https://github.com/evil\"\n  >x</a>",
        "&lt;a href=\"https://github.com/evil\"&gt;x&lt;/a&gt;",
    ] {
        let notes = format_release_notes(body).unwrap_or_default();
        let lowered = notes.to_ascii_lowercase();
        let squeezed: String = lowered.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(!squeezed.contains("<a"), "{body:?} -> {notes:?}");
        assert!(!squeezed.contains("</a"), "{body:?} -> {notes:?}");
    }
}

#[test]
fn dialog_content_cannot_carry_link_markup() {
    let copy = dialog_copy(
        &release("v2026.11.24.0", None),
        "<a href=\"https://github.com/evil\">1</a>",
    );
    assert!(!copy.content.to_ascii_lowercase().contains("<a"));
}

#[test]
fn release_notes_normalize_bullets() {
    let body = "- one\n* two\n+ three\n  - nested\n-notabullet\n- <img src=x>\n- four";
    assert_eq!(
        format_release_notes(body).as_deref(),
        Some(
            "\u{2022} one\n\u{2022} two\n\u{2022} three\n  \u{2022} nested\n-notabullet\n\n\u{2022} four"
        )
    );
}

#[test]
fn release_notes_collapse_blank_lines_and_rules() {
    let body = "\n\n First\r\n\r\n\r\n\r\n---\nSecond\n\n\n";
    assert_eq!(
        format_release_notes(body).as_deref(),
        Some("First\n\nSecond")
    );
    assert_eq!(
        format_release_notes("One\rTwo\r\nThree").as_deref(),
        Some("One\nTwo\nThree")
    );
}

#[test]
fn release_notes_are_none_when_nothing_is_left() {
    for body in [
        "",
        "   \n\t",
        "<!-- only -->",
        "![a](b)",
        "---",
        "[![a](b)](c)\n\n<a href=\"x\"></a>",
        "<p align=\"center\"><img src=\"x\"></p>\n\n- \n",
    ] {
        assert_eq!(format_release_notes(body), None, "{body:?}");
    }
}

#[test]
fn release_notes_cap_the_line_count() {
    let body: String = (1..=25).map(|n| format!("- item {n}\n")).collect();
    let notes = format_release_notes(&body).unwrap_or_default();
    assert_eq!(notes.lines().count(), update_dialog::MAX_NOTES_LINES);
    assert!(notes.ends_with("\u{2022} item 9\n\u{2026}"));
    assert!(!notes.contains("item 10"));
    let exact: String = (1..=10).map(|n| format!("- item {n}\n")).collect();
    let whole = format_release_notes(&exact).unwrap_or_default();
    assert!(whole.ends_with("\u{2022} item 10"));
    assert!(!whole.contains(update_dialog::ELLIPSIS));
}

#[test]
fn release_notes_do_not_end_on_a_blank_line_before_the_ellipsis() {
    let body: String = (1..=5).map(|n| format!("Section {n}\n\n")).collect();
    let notes = format_release_notes(&body).unwrap_or_default();
    assert_eq!(notes.lines().count(), update_dialog::MAX_NOTES_LINES - 1);
    assert!(notes.ends_with("Section 5"));
    let long: String = (1..=6).map(|n| format!("Section {n}\n\n")).collect();
    let capped = format_release_notes(&long).unwrap_or_default();
    assert!(capped.ends_with("Section 5\n\u{2026}"), "{capped:?}");
}

#[test]
fn release_notes_cap_the_length_at_a_word_boundary() {
    let body = format!("{}\nnext", "word ".repeat(200));
    let notes = format_release_notes(&body).unwrap_or_default();
    assert!(notes.chars().count() <= update_dialog::MAX_NOTES_CHARS);
    assert!(notes.ends_with("word\u{2026}"));
    assert!(!notes.contains("next"));
}

#[test]
fn release_notes_cut_long_words_without_losing_the_line() {
    let spaceless = "x".repeat(2000);
    let capped = format_release_notes(&spaceless).unwrap_or_default();
    assert_eq!(capped.chars().count(), update_dialog::MAX_NOTES_CHARS);
    assert!(capped.ends_with('\u{2026}'));
    let lopsided = format!("\u{2022} {}", "y".repeat(2000));
    let kept = format_release_notes(&lopsided).unwrap_or_default();
    assert!(kept.chars().count() > update_dialog::MAX_NOTES_CHARS / 2);
}

#[test]
fn release_notes_cut_multibyte_text_on_char_boundaries() {
    for unit in [
        "\u{1F680}",
        "\u{e9}",
        "\u{65E5}\u{672C}",
        "\u{1F469}\u{200D}\u{1F4BB} ",
    ] {
        let body = unit.repeat(1000);
        let notes = format_release_notes(&body).unwrap_or_default();
        assert!(
            notes.chars().count() <= update_dialog::MAX_NOTES_CHARS,
            "{unit}"
        );
        assert!(notes.ends_with(update_dialog::ELLIPSIS), "{unit}");
    }
}

#[test]
fn huge_release_bodies_stay_capped() {
    let body: String = (0..20_000)
        .map(|n| format!("- change {n} with **bold** and [link](u)\n"))
        .collect();
    let notes = format_release_notes(&body).unwrap_or_default();
    assert!(notes.chars().count() <= update_dialog::MAX_NOTES_CHARS);
    assert!(notes.lines().count() <= update_dialog::MAX_NOTES_LINES);
}

#[test]
fn short_release_notes_are_left_whole() {
    let body = "Fixes a crash.\nSpeeds up startup.";
    assert_eq!(format_release_notes(body).as_deref(), Some(body));
}
