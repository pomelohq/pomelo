; An element's children indent from the end of its start tag; a missing end tag (implied close)
; lets the block run to the element's end.
(element (start_tag) @start (end_tag)? @end) @indent
(script_element (start_tag) @start (end_tag) @end) @indent
(style_element (start_tag) @start (end_tag) @end) @indent
(start_tag ">" @end) @indent
(self_closing_tag "/>" @end) @indent
