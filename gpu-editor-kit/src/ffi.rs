//! C ABI for embedding the editor in a host app (the Swift/AppKit shell). The host owns an NSView backed by a
//! `CAMetalLayer`, hands its pointer in once, then drives render/resize/input from AppKit callbacks.

use std::ffi::c_void;
use std::slice;

use crate::{EditorBuffer, EditorRenderer};

pub struct Editor {
    renderer: EditorRenderer,
    buffer: EditorBuffer,
}

/// # Safety
/// `layer` must be a valid `CAMetalLayer` that outlives the returned handle. Returns null on GPU init failure.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_new(layer: *mut c_void, width: u32, height: u32, scale: f32) -> *mut Editor {
    let renderer = match EditorRenderer::from_metal_layer(layer, width, height, scale) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("pomelo-editor-kit: init failed: {e}");
            return std::ptr::null_mut();
        }
    };
    Box::into_raw(Box::new(Editor { renderer, buffer: EditorBuffer::default() }))
}

/// # Safety
/// `ed` must come from `pomelo_editor_new`; `ptr`/`len` a valid UTF-8 byte range.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_set_text(ed: *mut Editor, ptr: *const u8, len: usize) {
    let Some(ed) = ed.as_mut() else { return };
    let bytes = slice::from_raw_parts(ptr, len);
    if let Ok(text) = std::str::from_utf8(bytes) {
        ed.buffer = EditorBuffer::from_str(text);
        ed.renderer.follow_cursor(&ed.buffer);
    }
}

/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_render(ed: *mut Editor) {
    let Some(ed) = ed.as_mut() else { return };
    let _ = ed.renderer.render(&ed.buffer);
}

/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_resize(ed: *mut Editor, width: u32, height: u32) {
    let Some(ed) = ed.as_mut() else { return };
    ed.renderer.resize(width, height);
}

/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_scroll(ed: *mut Editor, delta_y: f32) {
    let Some(ed) = ed.as_mut() else { return };
    ed.renderer.scroll_by(delta_y, &ed.buffer);
}

/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_set_caret_on(ed: *mut Editor, on: bool) {
    let Some(ed) = ed.as_mut() else { return };
    ed.renderer.set_caret_on(on);
}

/// # Safety `ed` must come from `pomelo_editor_new`; `ptr`/`len` a valid UTF-8 byte range.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_insert_text(ed: *mut Editor, ptr: *const u8, len: usize) {
    let Some(ed) = ed.as_mut() else { return };
    let bytes = slice::from_raw_parts(ptr, len);
    if let Ok(text) = std::str::from_utf8(bytes) {
        for ch in text.chars() {
            if ch == '\n' || ch == '\t' || !ch.is_control() {
                ed.buffer.insert_char(ch);
            }
        }
        ed.renderer.follow_cursor(&ed.buffer);
        ed.renderer.set_caret_on(true);
    }
}

/// Special keys: 1 backspace, 2 enter, 3 left, 4 right, 5 up, 6 down.
/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_key(ed: *mut Editor, key: u32) {
    let Some(ed) = ed.as_mut() else { return };
    match key {
        1 => ed.buffer.backspace(),
        2 => ed.buffer.insert_char('\n'),
        3 => ed.buffer.move_left(),
        4 => ed.buffer.move_right(),
        5 => ed.buffer.move_up(),
        6 => ed.buffer.move_down(),
        _ => {}
    }
    ed.renderer.follow_cursor(&ed.buffer);
    ed.renderer.set_caret_on(true);
}

/// # Safety `ed` must come from `pomelo_editor_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_free(ed: *mut Editor) {
    if !ed.is_null() {
        drop(Box::from_raw(ed));
    }
}
