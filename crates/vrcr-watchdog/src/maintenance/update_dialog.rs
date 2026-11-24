use std::ptr::{self, null_mut};
use std::sync::LazyLock;

use regex_lite::Regex;
use tokio_util::sync::CancellationToken;
use vrcr_shared::console::{Component, Ux};
use windows_sys::Win32::Foundation::{HWND, LPARAM, S_OK, WPARAM};
use windows_sys::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_FLAGS, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TD_INFORMATION_ICON,
    TDF_ALLOW_DIALOG_CANCELLATION, TDF_CALLBACK_TIMER, TDF_ENABLE_HYPERLINKS,
    TDF_EXPANDED_BY_DEFAULT, TDF_USE_COMMAND_LINKS, TDM_CLICK_BUTTON, TDN_BUTTON_CLICKED,
    TDN_CREATED, TDN_HYPERLINK_CLICKED, TDN_TIMER, TaskDialogIndirect,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNORMAL, SendMessageW};
use windows_sys::core::HRESULT;

use super::dialog_owner::{attach_to_console, detach};
use super::update::{REPO, Release};
use super::update_prompt::normalize_version;

pub const TITLE: &str = "VRCResolver Update";
pub const UPDATE_NOW_BUTTON: i32 = 1001;
pub const LATER_BUTTON: i32 = 1002;
pub const SKIP_VERSION_BUTTON: i32 = 1003;
pub const SHOW_NOTES_TEXT: &str = "Show what's new";
pub const HIDE_NOTES_TEXT: &str = "Hide what's new";
pub const NOTES_LINK_TEXT: &str = "View full release notes on GitHub";
pub const MAX_NOTES_LINES: usize = 10;
pub const MAX_NOTES_CHARS: usize = 600;
pub const ELLIPSIS: char = '\u{2026}';
const MIN_PARTIAL_CHARS: usize = 20;
const MAX_LINK_CHARS: usize = 2048;
const SHELL_ERROR_LIMIT: usize = 32;
pub const FLAGS: TASKDIALOG_FLAGS =
    TDF_USE_COMMAND_LINKS | TDF_ALLOW_DIALOG_CANCELLATION | TDF_ENABLE_HYPERLINKS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateDecision {
    UpdateNow,
    Later,
    SkipVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogButton {
    pub id: i32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogCopy {
    pub title: String,
    pub instruction: String,
    pub content: String,
    pub buttons: Vec<DialogButton>,
    pub default_button: i32,
    pub notes: Option<String>,
    pub footer: Option<String>,
}

fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("release note pattern is valid")
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?s)<!--.*?(?:-->|$)"));
static LINKED_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\[!\[[^\]]*\]\([^)]*\)\]\([^)]*\)"));
static IMAGE: LazyLock<Regex> = LazyLock::new(|| pattern(r"!\[[^\]]*\]\([^)]*\)"));
static LINK: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([^\]]+)\]\([^)]*\)"));
static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"(?i)</?(?:a|abbr|b|blockquote|br|code|dd|del|details|div|dl|dt|em|h[1-6]|hr|i|img|ins|kbd|li|ol|p|picture|pre|s|source|span|strong|sub|summary|sup|table|tbody|td|th|thead|tr|u|ul|video)\b[^>]*>",
    )
});
static LINK_MARKUP: LazyLock<Regex> = LazyLock::new(|| pattern(r"<(\s*/?\s*[aA])"));
static FENCE: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s{0,3}(?:```|~~~)"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s{0,3}(?:>\s?)+"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s{0,3}#{1,6}\s+"));
static HEADING_CLOSE: LazyLock<Regex> = LazyLock::new(|| pattern(r"\s+#+\s*$"));
static BOLD: LazyLock<Regex> = LazyLock::new(|| pattern(r"\*\*([^*]+)\*\*"));
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| pattern(r"\\([\\`*_{}\[\]()#+.!<>|~-])"));
static BULLET: LazyLock<Regex> = LazyLock::new(|| pattern(r"^(\s*)[-*+](?:\s+|$)"));
static RULE: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\s*(?:-{3,}|\*{3,}|_{3,})\s*$"));

pub fn releases_page_url() -> String {
    format!("https://github.com/{REPO}/releases")
}

fn is_tag_segment(segment: &str) -> bool {
    segment.chars().any(|c| c.is_ascii_alphanumeric())
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

pub fn release_tag_url(tag: &str) -> Option<String> {
    is_tag_segment(tag).then(|| format!("{}/tag/{tag}", releases_page_url()))
}

pub fn is_allowed_release_url(url: &str) -> bool {
    url.strip_prefix(releases_page_url().as_str())
        .is_some_and(|rest| {
            rest.is_empty() || rest.strip_prefix("/tag/").is_some_and(is_tag_segment)
        })
}

fn without_link_markup(text: &str) -> String {
    LINK_MARKUP.replace_all(text, "\u{2039}$1").into_owned()
}

fn strip_tags(text: &str) -> String {
    let mut text = text.to_owned();
    loop {
        let stripped = HTML_TAG.replace_all(&text, "").into_owned();
        if stripped == text {
            return text;
        }
        text = stripped;
    }
}

fn clean_line(raw: &str) -> String {
    let line: String = raw.chars().filter(|c| !c.is_control()).collect();
    let line = QUOTE.replace(&line, "");
    let is_heading = HEADING.is_match(&line);
    let line = HEADING.replace(&line, "");
    let line = if is_heading {
        HEADING_CLOSE.replace(&line, "")
    } else {
        line
    };
    let line = BOLD.replace_all(line.trim_end(), "$1");
    let line = line.replace("**", "").replace('`', "");
    let line = ESCAPE.replace_all(&line, "$1");
    match BULLET.captures(&line) {
        Some(found) => {
            let item = line[found[0].len()..].trim_end();
            if item.is_empty() {
                return String::new();
            }
            let indent = if found[1].len() >= 2 { "  " } else { "" };
            format!("{indent}\u{2022} {item}")
        }
        None => line.trim().to_owned(),
    }
}

fn truncate_at_word(line: &str, budget: usize) -> String {
    let cut: String = line.chars().take(budget).collect();
    let kept = match cut.rsplit_once(' ') {
        Some((head, _)) if head.chars().count() * 2 >= budget => head,
        _ => cut.as_str(),
    };
    kept.trim_end().to_owned()
}

fn cap_notes(lines: &[String]) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0;
    let mut partial = None;
    let mut cut_short = false;
    for line in lines {
        if kept.len() >= MAX_NOTES_LINES {
            cut_short = true;
            break;
        }
        let separator = usize::from(!kept.is_empty());
        let cost = separator + line.chars().count();
        if used + cost + 2 > MAX_NOTES_CHARS {
            let room = MAX_NOTES_CHARS.saturating_sub(used + separator + 1);
            if room >= MIN_PARTIAL_CHARS {
                partial = Some(truncate_at_word(line, room));
            }
            cut_short = true;
            break;
        }
        used += cost;
        kept.push(line.clone());
    }
    if let Some(mut line) = partial {
        line.push(ELLIPSIS);
        kept.push(line);
    } else if cut_short {
        kept.truncate(MAX_NOTES_LINES - 1);
        while kept.last().is_some_and(String::is_empty) {
            kept.pop();
        }
        kept.push(ELLIPSIS.to_string());
    }
    kept.join("\n")
}

pub fn format_release_notes(body: &str) -> Option<String> {
    let text = body.replace("\r\n", "\n").replace('\r', "\n");
    let text = HTML_COMMENT.replace_all(&text, "");
    let text = LINKED_IMAGE.replace_all(&text, "");
    let text = IMAGE.replace_all(&text, "");
    let text = LINK.replace_all(&text, "$1");
    let text = strip_tags(&text);
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        if lines.len() > MAX_NOTES_LINES && lines.last().is_some_and(|line| !line.is_empty()) {
            break;
        }
        if RULE.is_match(raw) || FENCE.is_match(raw) {
            continue;
        }
        let line = clean_line(raw);
        if line.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    Some(without_link_markup(&cap_notes(&lines)))
}

fn footer_markup(url: &str) -> Option<String> {
    is_allowed_release_url(url).then(|| format!("<a href=\"{url}\">{NOTES_LINK_TEXT}</a>"))
}

pub fn dialog_flags(copy: &DialogCopy) -> TASKDIALOG_FLAGS {
    if copy.notes.is_some() {
        FLAGS | TDF_EXPANDED_BY_DEFAULT
    } else {
        FLAGS
    }
}

pub fn dialog_copy(release: &Release, current: &str) -> DialogCopy {
    let version = normalize_version(&release.tag);
    let current = current.trim();
    let content = if current.is_empty() {
        "Installing will restart VRCResolver.".to_owned()
    } else {
        without_link_markup(&format!(
            "You're running {current}. Installing will restart VRCResolver."
        ))
    };
    let button = |id: i32, text: String| DialogButton { id, text };
    DialogCopy {
        title: TITLE.to_owned(),
        instruction: format!("VRCResolver {version} is available"),
        content,
        buttons: vec![
            button(
                UPDATE_NOW_BUTTON,
                format!("Update now\nDownload and install {version}"),
            ),
            button(
                LATER_BUTTON,
                "Not now\nAsk again next time VRCResolver starts".to_owned(),
            ),
            button(
                SKIP_VERSION_BUTTON,
                "Don't ask again for this version\nNotify again when a newer release comes out"
                    .to_owned(),
            ),
        ],
        default_button: UPDATE_NOW_BUTTON,
        notes: release.body.as_deref().and_then(format_release_notes),
        footer: footer_markup(&release.notes_url()),
    }
}

pub fn decision_for(button: i32) -> UpdateDecision {
    match button {
        UPDATE_NOW_BUTTON => UpdateDecision::UpdateNow,
        SKIP_VERSION_BUTTON => UpdateDecision::SkipVersion,
        _ => UpdateDecision::Later,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

struct CallbackContext<'a> {
    closing: &'a CancellationToken,
    ux: &'a Ux,
}

pub fn ask(
    release: &Release,
    current: &str,
    closing: &CancellationToken,
    ux: &Ux,
) -> UpdateDecision {
    if closing.is_cancelled() {
        return UpdateDecision::Later;
    }
    let copy = dialog_copy(release, current);
    let notes = copy.notes.as_deref().map(wide);
    let footer = copy.footer.as_deref().map(wide);
    let show_notes = wide(SHOW_NOTES_TEXT);
    let hide_notes = wide(HIDE_NOTES_TEXT);
    let optional =
        |text: &Option<Vec<u16>>| text.as_ref().map_or(ptr::null(), |text| text.as_ptr());
    let context = CallbackContext { closing, ux };
    let title = wide(&copy.title);
    let instruction = wide(&copy.instruction);
    let content = wide(&copy.content);
    let labels: Vec<Vec<u16>> = copy
        .buttons
        .iter()
        .map(|button| wide(&button.text))
        .collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = copy
        .buttons
        .iter()
        .zip(&labels)
        .map(|(button, label)| TASKDIALOG_BUTTON {
            nButtonID: button.id,
            pszButtonText: label.as_ptr(),
        })
        .collect();
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(std::mem::size_of::<TASKDIALOGCONFIG>())
            .expect("dialog config size fits a u32"),
        dwFlags: dialog_flags(&copy) | TDF_CALLBACK_TIMER,
        pszWindowTitle: title.as_ptr(),
        pszMainInstruction: instruction.as_ptr(),
        pszContent: content.as_ptr(),
        pszExpandedInformation: optional(&notes),
        pszCollapsedControlText: if notes.is_some() {
            show_notes.as_ptr()
        } else {
            ptr::null()
        },
        pszExpandedControlText: if notes.is_some() {
            hide_notes.as_ptr()
        } else {
            ptr::null()
        },
        pszFooter: optional(&footer),
        cButtons: u32::try_from(buttons.len()).expect("three buttons fit a u32"),
        pButtons: buttons.as_ptr(),
        nDefaultButton: copy.default_button,
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: TD_INFORMATION_ICON,
        },
        pfCallback: Some(dialog_callback),
        lpCallbackData: ptr::from_ref(&context).expose_provenance().cast_signed(),
        ..TASKDIALOGCONFIG::default()
    };
    let mut button = 0;
    // SAFETY: The config, NUL-terminated strings, button array, callback context and button output live until this modal call returns.
    let result = unsafe { TaskDialogIndirect(&config, &mut button, null_mut(), null_mut()) };
    if result < 0 {
        ux.log_warn(
            Component::Update,
            &format!(
                "Could not open the update dialog: 0x{:08X}",
                result.cast_unsigned()
            ),
        );
        return UpdateDecision::Later;
    }
    if closing.is_cancelled() {
        return UpdateDecision::Later;
    }
    decision_for(button)
}

fn read_link(link: *const u16) -> Option<String> {
    if link.is_null() {
        return None;
    }
    let mut units = Vec::new();
    for offset in 0..MAX_LINK_CHARS {
        // SAFETY: TaskDialog passes a NUL-terminated string and the read stops at the terminator or the length cap.
        let unit = unsafe { *link.add(offset) };
        if unit == 0 {
            return String::from_utf16(&units).ok();
        }
        units.push(unit);
    }
    None
}

fn open_link(window: HWND, link: *const u16, ux: &Ux) {
    let Some(url) = read_link(link).filter(|url| is_allowed_release_url(url)) else {
        return;
    };
    let verb = wide("open");
    let target = wide(&url);
    // SAFETY: The verb and target are NUL-terminated and outlive the call; the other pointers are null.
    let result = unsafe {
        ShellExecuteW(
            window,
            verb.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.addr() <= SHELL_ERROR_LIMIT {
        ux.log_warn(
            Component::Update,
            &format!("Could not open the release notes: code {}", result.addr()),
        );
    }
}

unsafe extern "system" fn dialog_callback(
    window: HWND,
    notification: u32,
    _wparam: WPARAM,
    lparam: LPARAM,
    callback_data: isize,
) -> HRESULT {
    // SAFETY: TaskDialogIndirect passes back the exposed pointer to the context that outlives the modal call.
    let context = unsafe {
        &*ptr::with_exposed_provenance::<CallbackContext<'_>>(callback_data.cast_unsigned())
    };
    match notification.cast_signed() {
        TDN_CREATED => {
            let owner = attach_to_console(window);
            context.ux.log_write(
                Component::Update,
                &format!("update dialog owner: {owner:?}"),
            );
        }
        TDN_TIMER if context.closing.is_cancelled() => {
            let later = WPARAM::try_from(LATER_BUTTON).expect("button ID fits WPARAM");
            // SAFETY: The timer supplies a live task dialog and TDM_CLICK_BUTTON takes a scalar button ID.
            unsafe { SendMessageW(window, TDM_CLICK_BUTTON.cast_unsigned(), later, 0) };
        }
        TDN_BUTTON_CLICKED => detach(window),
        TDN_HYPERLINK_CLICKED => open_link(
            window,
            ptr::with_exposed_provenance::<u16>(lparam.cast_unsigned()),
            context.ux,
        ),
        _ => {}
    }
    S_OK
}
