use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOTOWNER, GWLP_HWNDPARENT, GetAncestor, GetClassNameW, IsIconic, IsWindow, IsWindowVisible,
    SetWindowLongPtrW,
};

const PSEUDO_CONSOLE_CLASS: &str = "PseudoConsoleWindow";
const CLASS_NAME_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowShown {
    Missing,
    Hidden,
    Minimized,
    Visible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleKind {
    Classic,
    Pseudo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsoleFacts {
    pub console: WindowShown,
    pub kind: ConsoleKind,
    pub root_owner: WindowShown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerChoice {
    Console,
    RootOwner,
    NoOwner,
}

pub fn choose_owner(facts: ConsoleFacts) -> OwnerChoice {
    if facts.console == WindowShown::Missing {
        return OwnerChoice::NoOwner;
    }
    let (candidate, choice) = match facts.kind {
        ConsoleKind::Classic => (facts.console, OwnerChoice::Console),
        ConsoleKind::Pseudo => (facts.root_owner, OwnerChoice::RootOwner),
    };
    match candidate {
        WindowShown::Visible => choice,
        WindowShown::Missing | WindowShown::Hidden | WindowShown::Minimized => OwnerChoice::NoOwner,
    }
}

fn shown(window: HWND) -> WindowShown {
    // SAFETY: IsWindow accepts any handle value, including null or stale ones.
    if window.is_null() || unsafe { IsWindow(window) } == 0 {
        return WindowShown::Missing;
    }
    // SAFETY: IsWindowVisible only reads window state and accepts any handle value.
    if unsafe { IsWindowVisible(window) } == 0 {
        return WindowShown::Hidden;
    }
    // SAFETY: IsIconic only reads window state and accepts any handle value.
    if unsafe { IsIconic(window) } != 0 {
        return WindowShown::Minimized;
    }
    WindowShown::Visible
}

fn kind_of(window: HWND) -> ConsoleKind {
    let mut buffer = [0u16; CLASS_NAME_CAPACITY];
    let capacity = i32::try_from(buffer.len()).expect("class name buffer length fits an i32");
    // SAFETY: The buffer is live and writable for `capacity` UTF-16 units, and GetClassNameW accepts any handle value.
    let written = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), capacity) };
    let written = usize::try_from(written)
        .unwrap_or_default()
        .min(buffer.len());
    if String::from_utf16_lossy(&buffer[..written]) == PSEUDO_CONSOLE_CLASS {
        ConsoleKind::Pseudo
    } else {
        ConsoleKind::Classic
    }
}

fn observe() -> (ConsoleFacts, HWND, HWND) {
    // SAFETY: GetConsoleWindow takes no arguments and returns null when the process has no console.
    let console = unsafe { GetConsoleWindow() };
    let console_shown = shown(console);
    if console_shown == WindowShown::Missing {
        let facts = ConsoleFacts {
            console: console_shown,
            kind: ConsoleKind::Classic,
            root_owner: WindowShown::Missing,
        };
        return (facts, console, std::ptr::null_mut());
    }
    // SAFETY: GetAncestor only reads the owner chain and returns null for an invalid handle.
    let root = unsafe { GetAncestor(console, GA_ROOTOWNER) };
    let root_owner = if root == console {
        WindowShown::Missing
    } else {
        shown(root)
    };
    let facts = ConsoleFacts {
        console: console_shown,
        kind: kind_of(console),
        root_owner,
    };
    (facts, console, root)
}

pub(super) fn attach_to_console(dialog: HWND) -> OwnerChoice {
    let (facts, console, root) = observe();
    let choice = choose_owner(facts);
    let owner = match choice {
        OwnerChoice::Console => console,
        OwnerChoice::RootOwner => root,
        OwnerChoice::NoOwner => return choice,
    };
    // SAFETY: `dialog` is this thread's live top-level task dialog; GWLP_HWNDPARENT only records the owner handle.
    unsafe { SetWindowLongPtrW(dialog, GWLP_HWNDPARENT, owner.addr().cast_signed()) };
    choice
}

pub(super) fn detach(dialog: HWND) {
    // SAFETY: `dialog` is this thread's live top-level task dialog and a null owner removes the link.
    unsafe { SetWindowLongPtrW(dialog, GWLP_HWNDPARENT, 0) };
}
