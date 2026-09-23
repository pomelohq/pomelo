; Block types and attribute names first: the first pattern to capture a node wins.
(block (identifier) @type)
(attribute (identifier) @property)
(object_elem key: (expression (variable_expr (identifier) @property)))
(function_call (identifier) @function)
(get_attr (identifier) @property)
(variable_expr (identifier) @variable)

(comment) @comment
(string_lit) @string
(heredoc_template) @string
(heredoc_identifier) @punctuation.special
(template_interpolation_start) @punctuation.special
(template_interpolation_end) @punctuation.special
(numeric_lit) @number
(bool_lit) @boolean
(null_lit) @constant.builtin

["if" "else" "endif" "for" "endfor" "in"] @keyword
