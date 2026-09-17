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
        ed.renderer.mark_text_dirty();
        ed.renderer.reset_hscroll();
        ed.renderer.follow_cursor(&ed.buffer);
    }
}

/// Place the cursor at a point in logical view coords (a click). Collapses selection.
/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_click(ed: *mut Editor, x: f32, y: f32) {
    let Some(ed) = ed.as_mut() else { return };
    let (line, col) = ed.renderer.point_to_line_col(x, y);
    let off = ed.buffer.offset_at(line, col);
    ed.buffer.place_cursor(off);
    ed.renderer.set_caret_on(true);
}

/// Extend the selection to a point (a drag).
/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_drag(ed: *mut Editor, x: f32, y: f32) {
    let Some(ed) = ed.as_mut() else { return };
    let (line, col) = ed.renderer.point_to_line_col(x, y);
    let off = ed.buffer.offset_at(line, col);
    ed.buffer.extend_cursor(off);
    ed.renderer.set_caret_on(true);
}

/// Select the word at a point (a double-click).
/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_double_click(ed: *mut Editor, x: f32, y: f32) {
    let Some(ed) = ed.as_mut() else { return };
    let (line, col) = ed.renderer.point_to_line_col(x, y);
    let off = ed.buffer.offset_at(line, col);
    ed.buffer.select_word_at(off);
    ed.renderer.set_caret_on(true);
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
pub unsafe extern "C" fn pomelo_editor_scroll(ed: *mut Editor, delta_x: f32, delta_y: f32) {
    let Some(ed) = ed.as_mut() else { return };
    ed.renderer.scroll_by(delta_x, delta_y, &ed.buffer);
}

/// # Safety `ed` must come from `pomelo_editor_new`.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_set_caret_on(ed: *mut Editor, on: bool) {
    let Some(ed) = ed.as_mut() else { return };
    ed.renderer.set_caret_on(on);
}

/// Set the syntax language from a file extension (e.g. "tsx", "rs"). Unknown -> plain text.
/// # Safety `ed` must come from `pomelo_editor_new`; `ptr`/`len` a valid UTF-8 byte range.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_set_language(ed: *mut Editor, ptr: *const u8, len: usize) {
    let Some(ed) = ed.as_mut() else { return };
    let ext = std::str::from_utf8(slice::from_raw_parts(ptr, len)).unwrap_or("");
    ed.renderer.set_language(crate::highlight::Lang::from_ext(ext));
}

/// Set the tab-bar title (usually the file name).
/// # Safety `ed` must come from `pomelo_editor_new`; `ptr`/`len` a valid UTF-8 byte range.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_set_tab_title(ed: *mut Editor, ptr: *const u8, len: usize) {
    let Some(ed) = ed.as_mut() else { return };
    let title = std::str::from_utf8(slice::from_raw_parts(ptr, len)).unwrap_or("");
    ed.renderer.set_tab_title(title);
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
        ed.renderer.mark_text_dirty();
        ed.renderer.follow_cursor(&ed.buffer);
        ed.renderer.set_caret_on(true);
    }
}

/// Special keys: 1 backspace, 2 enter, 3 left, 4 right, 5 up, 6 down,
/// 7 undo, 8 redo, 9 select-all, 10 shift-left, 11 shift-right, 12 shift-up, 13 shift-down.
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
        7 => ed.buffer.undo(),
        8 => ed.buffer.redo(),
        9 => ed.buffer.select_all(),
        10 => ed.buffer.extend_left(),
        11 => ed.buffer.extend_right(),
        12 => ed.buffer.extend_up(),
        13 => ed.buffer.extend_down(),
        14 => ed.buffer.move_word_left(),
        15 => ed.buffer.move_word_right(),
        16 => ed.buffer.move_home(),
        17 => ed.buffer.move_end(),
        18 => ed.buffer.extend_word_left(),
        19 => ed.buffer.extend_word_right(),
        20 => ed.buffer.extend_home(),
        21 => ed.buffer.extend_end(),
        _ => {}
    }
    if matches!(key, 1 | 2 | 7 | 8) {
        ed.renderer.mark_text_dirty();
    }
    ed.renderer.follow_cursor(&ed.buffer);
    ed.renderer.set_caret_on(true);
}

/// Copy the selected text into `out` (up to `cap` bytes); returns the full byte length (may exceed cap; call again
/// with a bigger buffer, or pass cap=0 to just query the length). Empty selection returns 0.
/// # Safety `ed` must come from `pomelo_editor_new`; `out`/`cap` a writable byte range.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_copy(ed: *mut Editor, out: *mut u8, cap: usize) -> usize {
    let Some(ed) = ed.as_mut() else { return 0 };
    let Some(text) = ed.buffer.selected_text() else { return 0 };
    let bytes = text.as_bytes();
    if cap > 0 && !out.is_null() {
        let n = bytes.len().min(cap);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, n);
    }
    bytes.len()
}

/// # Safety `ed` must come from `pomelo_editor_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn pomelo_editor_free(ed: *mut Editor) {
    if !ed.is_null() {
        drop(Box::from_raw(ed));
    }
}
