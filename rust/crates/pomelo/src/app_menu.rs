//! The macOS menu bar. Window commands run the keymap's actions and show their current keys; Edit entries
//! replay their key into the focused window so the editor, terminal and text fields handle them as typed.

use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, Sel};
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSString};
use workspace::keymap::{Action, Keymap, Keystroke};

pub enum Command {
    Run(Action),
    Update(workspace::UpdateAction),
    Open(&'static str),
}

enum Entry {
    Command(&'static str, Command),
    /// Replays these keys into the focused window.
    Keys(&'static str, &'static str),
    Standard(&'static str, Sel, &'static str),
    Services,
    Separator,
}

const DOCS: &str = "https://pomelohq.app/docs/quickstart";
const SHORTCUTS: &str = "https://pomelohq.app/docs/shortcuts";
const ISSUES: &str = "https://github.com/pomelohq/pomelo/issues/new";

fn menus() -> Vec<(&'static str, Vec<Entry>)> {
    use workspace::UpdateAction;
    use Entry::{Keys, Separator, Services, Standard};
    let run = |title, action| Entry::Command(title, Command::Run(action));
    vec![
        (
            "Pomelo",
            vec![
                Standard("About Pomelo", sel!(orderFrontStandardAboutPanel:), ""),
                Entry::Command("Check for Updates...", Command::Update(UpdateAction::Check)),
                Entry::Command("Release Notes", Command::Update(UpdateAction::ReleaseNotes)),
                Separator,
                run("Settings...", Action::OpenSettings),
                run("Keymap", Action::OpenKeymap),
                Separator,
                Services,
                Separator,
                Standard("Hide Pomelo", sel!(hide:), "cmd-h"),
                Standard("Hide Others", sel!(hideOtherApplications:), "cmd-alt-h"),
                Standard("Show All", sel!(unhideAllApplications:), ""),
                Separator,
                Standard("Quit Pomelo", sel!(terminate:), "cmd-q"),
            ],
        ),
        (
            "File",
            vec![
                run("New Workspace", Action::NewWorkspace),
                run("New Project...", Action::NewProject),
                run("Open Project...", Action::OpenProject),
                Separator,
                Keys("Save", "cmd-s"),
                Separator,
                run("Close Tab", Action::CloseActiveItem),
                run("Close All Tabs", Action::CloseAllItems),
                Standard("Close Window", sel!(performClose:), "cmd-shift-w"),
            ],
        ),
        (
            "Edit",
            vec![
                Keys("Undo", "cmd-z"),
                Keys("Redo", "cmd-shift-z"),
                Separator,
                Keys("Cut", "cmd-x"),
                Keys("Copy", "cmd-c"),
                Keys("Paste", "cmd-v"),
                Keys("Select All", "cmd-a"),
                Separator,
                Keys("Find", "cmd-f"),
                run("Find in Project", Action::ProjectSearch),
            ],
        ),
        (
            "View",
            vec![
                run("Toggle Left Dock", Action::ToggleLeftDock),
                run("Toggle Right Dock", Action::ToggleRightDock),
                run("Toggle Bottom Dock", Action::ToggleBottomDock),
                Separator,
                run("Files", Action::FocusFiles),
                run("Git", Action::FocusGit),
                run("Services", Action::FocusServices),
                run("Database", Action::FocusDatabase),
                run("Pull Requests", Action::FocusPullRequests),
                Separator,
                run("Agent", Action::ToggleAgent),
                run("Terminal", Action::ToggleTerminal),
                Separator,
                Keys("Split Right", "cmd-\\"),
                run("Markdown Preview", Action::MarkdownPreview),
                Separator,
                run("Next Theme", Action::CycleTheme),
            ],
        ),
        (
            "Go",
            vec![
                run("Command Palette...", Action::CommandPalette),
                run("Go to File...", Action::FileFinder),
                Keys("Go to Line...", "ctrl-g"),
                Separator,
                Keys("Back", "ctrl--"),
                Keys("Forward", "ctrl-_"),
                Separator,
                run("Previous Tab", Action::ActivatePreviousTab),
                run("Next Tab", Action::ActivateNextTab),
                Separator,
                run("Switch Workspace...", Action::SwitchWorkspace),
            ],
        ),
        (
            "Window",
            vec![
                Standard("Minimize", sel!(performMiniaturize:), "cmd-m"),
                Standard("Zoom", sel!(performZoom:), ""),
                Separator,
                Standard("Bring All to Front", sel!(arrangeInFront:), ""),
            ],
        ),
        (
            "Help",
            vec![
                Entry::Command("Pomelo Docs", Command::Open(DOCS)),
                Entry::Command("Keyboard Shortcuts", Command::Open(SHORTCUTS)),
                Entry::Command("Release Notes", Command::Update(UpdateAction::ReleaseNotes)),
                Separator,
                Entry::Command("Report an Issue...", Command::Open(ISSUES)),
            ],
        ),
    ]
}

struct State {
    commands: Vec<Command>,
    keys: Vec<&'static str>,
    selected: Vec<usize>,
}

static STATE: Mutex<State> = Mutex::new(State {
    commands: Vec::new(),
    keys: Vec::new(),
    selected: Vec::new(),
});

/// Tags at or above this replay `keys` instead of queuing a command.
const KEYS_TAG: isize = 1 << 20;

declare_class!(
    struct Target;

    unsafe impl ClassType for Target {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "PomeloMenuTarget";
    }

    impl DeclaredClass for Target {}

    unsafe impl Target {
        #[method(menuItemSelected:)]
        fn menu_item_selected(&self, item: &NSMenuItem) {
            // SAFETY: a plain getter on the item AppKit just activated.
            let tag = unsafe { item.tag() };
            if tag >= KEYS_TAG {
                let keys = STATE
                    .lock()
                    .ok()
                    .and_then(|state| state.keys.get((tag - KEYS_TAG) as usize).copied());
                if let Some(keys) = keys {
                    replay(keys);
                }
                return;
            }
            if let Ok(mut state) = STATE.lock() {
                state.selected.push(tag as usize);
            }
            ui::wake();
        }
    }
);

static TARGET: Mutex<Option<TargetHandle>> = Mutex::new(None);

struct TargetHandle(Retained<Target>);

// SAFETY: the target is created and used on the main thread only; the mutex just keeps it alive.
unsafe impl Send for TargetHandle {}

fn target() -> Option<Retained<Target>> {
    let mut slot = TARGET.lock().ok()?;
    if slot.is_none() {
        // SAFETY: a plain NSObject subclass with no ivars.
        let target: Retained<Target> = unsafe { msg_send_id![Target::alloc(), init] };
        *slot = Some(TargetHandle(target));
    }
    slot.as_ref().map(|handle| handle.0.clone())
}

/// Build the menu bar (again) with each command's current key from `keymap`.
pub fn install(keymap: &Keymap) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(target) = target() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let bar = NSMenu::new(mtm);
    let mut commands = Vec::new();
    let mut replayed = Vec::new();
    for (title, entries) in menus() {
        // SAFETY: menus built and attached on the main thread.
        unsafe {
            let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
            for entry in entries {
                let item = match entry {
                    Entry::Separator => NSMenuItem::separatorItem(mtm),
                    Entry::Services => {
                        let services =
                            NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Services"));
                        let item = menu_item(mtm, "Services", None, "");
                        item.setSubmenu(Some(&services));
                        app.setServicesMenu(Some(&services));
                        item
                    }
                    Entry::Standard(title, action, keys) => {
                        menu_item(mtm, title, Some(action), keys)
                    }
                    Entry::Keys(title, keys) => {
                        let item = menu_item(mtm, title, Some(sel!(menuItemSelected:)), keys);
                        item.setTarget(Some(&target));
                        item.setTag(KEYS_TAG + replayed.len() as isize);
                        replayed.push(keys);
                        item
                    }
                    Entry::Command(title, command) => {
                        let keys = match &command {
                            Command::Run(action) => keymap.binding_for(*action),
                            _ => None,
                        };
                        let keys = keys.filter(|keys| !keys.contains(' ')).unwrap_or_default();
                        let item = menu_item(mtm, title, Some(sel!(menuItemSelected:)), &keys);
                        item.setTarget(Some(&target));
                        item.setTag(commands.len() as isize);
                        commands.push(command);
                        item
                    }
                };
                menu.addItem(&item);
            }
            let top = menu_item(mtm, title, None, "");
            top.setSubmenu(Some(&menu));
            bar.addItem(&top);
            match title {
                "Window" => app.setWindowsMenu(Some(&menu)),
                "Help" => app.setHelpMenu(Some(&menu)),
                _ => {}
            }
        }
    }
    app.setMainMenu(Some(&bar));
    if let Ok(mut state) = STATE.lock() {
        state.commands = commands;
        state.keys = replayed;
    }
}

/// What the user picked since the last call.
pub fn take_selected() -> Vec<Command> {
    let Ok(mut state) = STATE.lock() else {
        return Vec::new();
    };
    let picked = std::mem::take(&mut state.selected);
    let mut commands = Vec::new();
    for index in picked {
        let command = match state.commands.get(index) {
            Some(Command::Run(action)) => Command::Run(*action),
            Some(Command::Update(action)) => Command::Update(*action),
            Some(Command::Open(url)) => Command::Open(url),
            None => continue,
        };
        commands.push(command);
    }
    commands
}

/// # Safety
/// Main thread only.
unsafe fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    keys: &str,
) -> Retained<NSMenuItem> {
    let (equivalent, flags) = key_equivalent(keys);
    let item = NSMenuItem::initWithTitle_action_keyEquivalent(
        mtm.alloc(),
        &NSString::from_str(title),
        action,
        &NSString::from_str(&equivalent),
    );
    item.setKeyEquivalentModifierMask(flags);
    item
}

/// A binding as AppKit's key equivalent: the character and its modifier mask.
fn key_equivalent(keys: &str) -> (String, NSEventModifierFlags) {
    let Some(stroke) = Keystroke::parse(keys) else {
        return (String::new(), NSEventModifierFlags(0));
    };
    let key = match stroke.key.as_str() {
        "left" => "\u{f702}".to_string(),
        "right" => "\u{f703}".to_string(),
        "up" => "\u{f700}".to_string(),
        "down" => "\u{f701}".to_string(),
        "pageup" => "\u{f72c}".to_string(),
        "pagedown" => "\u{f72d}".to_string(),
        "enter" => "\r".to_string(),
        "escape" => "\u{1b}".to_string(),
        key if key.chars().count() == 1 => key.to_string(),
        _ => return (String::new(), NSEventModifierFlags(0)),
    };
    (key, modifier_flags(&stroke))
}

fn modifier_flags(stroke: &Keystroke) -> NSEventModifierFlags {
    let mut flags = NSEventModifierFlags(0);
    if stroke.cmd {
        flags |= NSEventModifierFlags::NSEventModifierFlagCommand;
    }
    if stroke.shift {
        flags |= NSEventModifierFlags::NSEventModifierFlagShift;
    }
    if stroke.alt {
        flags |= NSEventModifierFlags::NSEventModifierFlagOption;
    }
    if stroke.ctrl {
        flags |= NSEventModifierFlags::NSEventModifierFlagControl;
    }
    flags
}

/// US-layout virtual key codes for the keys the Edit and Go entries replay.
fn key_code(key: &str) -> Option<u16> {
    Some(match key {
        "a" => 0,
        "s" => 1,
        "f" => 3,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "-" | "_" => 27,
        "\\" => 42,
        _ => return None,
    })
}

const COMMAND_KEY_CODE: u16 = 55;
const SHIFT_KEY_CODE: u16 = 56;
const CONTROL_KEY_CODE: u16 = 59;

/// Send `keys` to the key window as if typed. From the keyboard the modifiers are already down; from a click
/// they are pressed and released around the key, so the window's modifier tracking sees them.
fn replay(keys: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(mut stroke) = Keystroke::parse(keys) else {
        return;
    };
    // `_` is typed as shift and minus.
    if stroke.key == "_" {
        stroke.shift = true;
    }
    let Some(code) = key_code(&stroke.key) else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app.keyWindow() else {
        return;
    };
    // SAFETY: event construction and delivery on the main thread, to a live window.
    unsafe {
        let from_keyboard = app
            .currentEvent()
            .is_some_and(|event| event.r#type() == NSEventType::KeyDown);
        let number = window.windowNumber();
        let flags = modifier_flags(&stroke);
        let modifier_keys: Vec<(bool, u16, NSEventModifierFlags)> = vec![
            (
                stroke.cmd,
                COMMAND_KEY_CODE,
                NSEventModifierFlags::NSEventModifierFlagCommand,
            ),
            (
                stroke.shift,
                SHIFT_KEY_CODE,
                NSEventModifierFlags::NSEventModifierFlagShift,
            ),
            (
                stroke.ctrl,
                CONTROL_KEY_CODE,
                NSEventModifierFlags::NSEventModifierFlagControl,
            ),
        ];
        let flags_event = |held: NSEventModifierFlags, code: u16| {
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::FlagsChanged,
                NSPoint::new(0.0, 0.0),
                held,
                0.0,
                number,
                None,
                &NSString::from_str(""),
                &NSString::from_str(""),
                false,
                code,
            )
        };
        let mut held = NSEventModifierFlags(0);
        if !from_keyboard {
            for (pressed, code, flag) in &modifier_keys {
                if *pressed {
                    held |= *flag;
                    if let Some(event) = flags_event(held, *code) {
                        window.sendEvent(&event);
                    }
                }
            }
        }
        let characters = NSString::from_str(&stroke.key);
        for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                kind,
                NSPoint::new(0.0, 0.0),
                flags,
                0.0,
                number,
                None,
                &characters,
                &characters,
                false,
                code,
            );
            if let Some(event) = event {
                window.sendEvent(&event);
            }
        }
        if !from_keyboard {
            for (pressed, code, flag) in modifier_keys.iter().rev() {
                if *pressed {
                    held &= !*flag;
                    if let Some(event) = flags_event(held, *code) {
                        window.sendEvent(&event);
                    }
                }
            }
        }
    }
}

pub fn open_url(url: &str) {
    if let Err(error) = std::process::Command::new("open").arg(url).spawn() {
        eprintln!("open {url}: {error}");
    }
}

fn replayed_strokes() -> Vec<(String, NSEventModifierFlags)> {
    menus()
        .into_iter()
        .flat_map(|(_, entries)| entries)
        .filter_map(|entry| match entry {
            Entry::Keys(_, keys) => {
                let stroke = Keystroke::parse(keys)?;
                Some((stroke.key.to_lowercase(), modifier_flags(&stroke)))
            }
            _ => None,
        })
        .collect()
}

static REPLAYED: Mutex<Vec<(String, NSEventModifierFlags)>> = Mutex::new(Vec::new());

pub fn route_edit_keys_to_window(window: &winit::window::Window) {
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    if let Ok(mut replayed) = REPLAYED.lock() {
        if replayed.is_empty() {
            *replayed = replayed_strokes();
        }
    }
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    extern "C" fn perform_key_equivalent(this: &AnyObject, _: Sel, event: &NSEvent) -> Bool {
        let relevant = NSEventModifierFlags::NSEventModifierFlagCommand
            | NSEventModifierFlags::NSEventModifierFlagShift
            | NSEventModifierFlags::NSEventModifierFlagOption
            | NSEventModifierFlags::NSEventModifierFlagControl;
        let (key, flags) = unsafe {
            let key = event
                .charactersIgnoringModifiers()
                .map(|key| key.to_string().to_lowercase())
                .unwrap_or_default();
            (key, event.modifierFlags() & relevant)
        };
        let ours = REPLAYED
            .lock()
            .is_ok_and(|replayed| replayed.iter().any(|(k, f)| *k == key && *f == flags));
        if ours {
            unsafe {
                let _: () = objc2::msg_send![this, keyDown: event];
            }
            return Bool::YES;
        }
        Bool::NO
    }
    unsafe {
        let view = handle.ns_view.as_ptr() as *mut AnyObject;
        let class: *const AnyClass = objc2::ffi::object_getClass(view.cast()).cast();
        if class.is_null() {
            return;
        }
        let imp: Imp = std::mem::transmute(
            perform_key_equivalent as extern "C" fn(&AnyObject, Sel, &NSEvent) -> Bool,
        );
        objc2::ffi::class_addMethod(
            class as *mut _,
            sel!(performKeyEquivalent:).as_ptr(),
            Some(imp),
            c"c@:@".as_ptr(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_become_key_equivalents() {
        let (key, flags) = key_equivalent("cmd-shift-c");
        assert_eq!(key, "c");
        assert!(flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand));
        assert!(flags.contains(NSEventModifierFlags::NSEventModifierFlagShift));
        assert_eq!(key_equivalent("ctrl-`").0, "`");
        assert_eq!(key_equivalent("cmd-alt-left").0, "\u{f702}");
        assert_eq!(key_equivalent("f13").0, "");
    }

    #[test]
    fn every_replayed_key_has_a_key_code() {
        for (_, entries) in menus() {
            for entry in entries {
                if let Entry::Keys(title, keys) = entry {
                    let stroke = Keystroke::parse(keys).unwrap_or_default();
                    assert!(key_code(&stroke.key).is_some(), "{title}: {keys}");
                }
            }
        }
    }

    #[test]
    fn the_edit_keys_are_taken_before_the_menu_bar() {
        let strokes = replayed_strokes();
        let command = NSEventModifierFlags::NSEventModifierFlagCommand;
        let shift = NSEventModifierFlags::NSEventModifierFlagShift;
        assert!(strokes.contains(&("v".to_string(), command)));
        assert!(strokes.contains(&("z".to_string(), command | shift)));
        assert!(
            !strokes.iter().any(|(key, _)| key == "q"),
            "window commands stay with the menu"
        );
    }
}
