#ifndef POMELO_EDITOR_H
#define POMELO_EDITOR_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

// C ABI for pomelo-editor-kit (Rust + wgpu). The Swift host owns a CAMetalLayer-backed NSView,
// hands its layer pointer to pomelo_editor_new once, then drives render/resize/input.

typedef struct Editor Editor;

// layer: a CAMetalLayer*. Returns NULL on GPU init failure.
Editor *pomelo_editor_new(void *layer, uint32_t width, uint32_t height, float scale);

void pomelo_editor_set_text(Editor *ed, const uint8_t *ptr, size_t len);
void pomelo_editor_render(Editor *ed);
void pomelo_editor_resize(Editor *ed, uint32_t width, uint32_t height);
void pomelo_editor_scroll(Editor *ed, float delta_x, float delta_y);
void pomelo_editor_set_caret_on(Editor *ed, bool on);
void pomelo_editor_set_language(Editor *ed, const uint8_t *ptr, size_t len);
void pomelo_editor_set_tab_title(Editor *ed, const uint8_t *ptr, size_t len);
void pomelo_editor_click(Editor *ed, float x, float y);
void pomelo_editor_drag(Editor *ed, float x, float y);
void pomelo_editor_insert_text(Editor *ed, const uint8_t *ptr, size_t len);

// Special keys: 1 backspace, 2 enter, 3 left, 4 right, 5 up, 6 down, 7 undo, 8 redo,
// 9 select-all, 10 shift-left, 11 shift-right, 12 shift-up, 13 shift-down, 14 word-left,
// 15 word-right, 16 home, 17 end, 18 shift-word-left, 19 shift-word-right, 20 shift-home, 21 shift-end,
// 22 collapse-cursors (Esc), 23 select-next (Cmd+D).
void pomelo_editor_key(Editor *ed, uint32_t key);
void pomelo_editor_double_click(Editor *ed, float x, float y);
void pomelo_editor_add_cursor(Editor *ed, float x, float y);

// Copy selection into out (up to cap bytes); returns full byte length. cap=0 queries length.
size_t pomelo_editor_copy(Editor *ed, uint8_t *out, size_t cap);
void pomelo_editor_free(Editor *ed);

#endif
