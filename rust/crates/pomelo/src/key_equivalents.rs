//! AppKit handles Command key presses as key equivalents before the window sees them, and swallows some
//! outright (Cmd+? opens the Help menu's search). A local event monitor sees each key press first, so a press
//! the keymap binds is taken here and handed to the app instead.

use std::ptr::NonNull;
use std::sync::Mutex;

use block2::RcBlock;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
use workspace::keymap::{KeyMatch, Keymap, Keystroke};

#[derive(Default)]
struct Shared {
    keymap: Option<Keymap>,
    main_focused: bool,
    chord_pending: bool,
    taken: Vec<Keystroke>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    keymap: None,
    main_focused: true,
    chord_pending: false,
    taken: Vec::new(),
});

pub fn install() {
    let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event for the duration of the call.
        let event_ref = unsafe { event.as_ref() };
        if take(event_ref) {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });
    // SAFETY: called on the main thread at launch; the monitor lives as long as the app.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    };
    // The monitor is removed only when the app quits, so its handle is kept for good.
    std::mem::forget(monitor);
}

fn take(event: &NSEvent) -> bool {
    // SAFETY: plain property reads on a live key event.
    let (flags, characters) =
        unsafe { (event.modifierFlags(), event.charactersIgnoringModifiers()) };
    if !flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand) {
        return false;
    }
    let Some(characters) = characters.map(|text| text.to_string()) else {
        return false;
    };
    if characters.chars().count() != 1 || characters.chars().any(char::is_control) {
        return false;
    }
    let stroke = Keystroke {
        cmd: true,
        ctrl: flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
        alt: flags.contains(NSEventModifierFlags::NSEventModifierFlagOption),
        shift: flags.contains(NSEventModifierFlags::NSEventModifierFlagShift),
        key: characters.to_lowercase(),
    };
    let Ok(mut shared) = SHARED.lock() else {
        return false;
    };
    if shared.chord_pending || !shared.main_focused {
        return false;
    }
    let bound = shared
        .keymap
        .as_ref()
        .is_some_and(|keymap| matches!(keymap.match_keys(&[], &stroke), KeyMatch::Action(_)));
    if bound {
        shared.taken.push(stroke);
        drop(shared);
        ui::wake();
    }
    bound
}

pub fn set_keymap(keymap: &Keymap) {
    if let Ok(mut shared) = SHARED.lock() {
        shared.keymap = Some(keymap.clone());
    }
}

pub fn set_main_focused(focused: bool) {
    if let Ok(mut shared) = SHARED.lock() {
        shared.main_focused = focused;
    }
}

pub fn set_chord_pending(pending: bool) {
    if let Ok(mut shared) = SHARED.lock() {
        shared.chord_pending = pending;
    }
}

pub fn take_pending() -> Vec<Keystroke> {
    SHARED
        .lock()
        .map(|mut shared| std::mem::take(&mut shared.taken))
        .unwrap_or_default()
}
