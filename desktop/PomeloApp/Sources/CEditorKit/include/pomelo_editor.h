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

// Special keys: 1 backspace, 2 enter, 3 left, 4 right, 5 up, 6 down.
void pomelo_editor_key(Editor *ed, uint32_t key);
void pomelo_editor_free(Editor *ed);

#endif
