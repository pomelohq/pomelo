; Declarations and calls first: the first pattern to capture a node wins.
(function_declaration name: (_) @function)
(class_declaration name: (_) @type)
(call_expression (identifier) @function)
(call_expression (navigation_expression (identifier) @function.method .))
(user_type (identifier) @type)
(annotation) @attribute
(label) @label
(this_expression) @variable.special
(super_expression) @variable.special
(package_header (qualified_identifier) @namespace)
(import (qualified_identifier) @namespace)
(parameter (identifier) @variable.parameter)

(line_comment) @comment
(block_comment) @comment
(string_literal) @string
(multiline_string_literal) @string
(character_literal) @string
(escape_sequence) @string.escape
(interpolation) @embedded
(number_literal) @number
(float_literal) @number

[
  "abstract" "actual" "annotation" "as" "by" "catch" "class" "companion" "const" "constructor"
  "crossinline" "data" "do" "else" "enum" "expect" "external" "final" "finally" "for" "fun" "get"
  "if" "import" "in" "infix" "init" "inline" "inner" "interface" "internal" "is" "lateinit"
  "noinline" "object" "open" "operator" "out" "override" "package" "private" "protected" "public"
  "return" "sealed" "set" "super" "suspend" "tailrec" "this" "throw" "try" "typealias" "val"
  "value" "var" "vararg" "when" "where" "while"
] @keyword
