; Definitions and references first: the first pattern to capture a node wins.
(named_type (name) @type)
(object_type_definition (name) @type)
(interface_type_definition (name) @type)
(enum_type_definition (name) @type)
(input_object_type_definition (name) @type)
(scalar_type_definition (name) @type)
(union_type_definition (name) @type)
(field_definition (name) @property)
(field (name) @property)
(alias (name) @property)
(argument (name) @variable.parameter)
(input_value_definition (name) @variable.parameter)
(operation_definition (name) @function)
(fragment_name (name) @function)
(directive "@" @attribute (name) @attribute)
(variable) @variable
(enum_value) @constant

(comment) @comment
(description) @comment.doc
(string_value) @string
(int_value) @number
(float_value) @number
(boolean_value) @boolean
(null_value) @constant.builtin

[
  "directive" "enum" "extend" "fragment" "implements" "input" "interface" "mutation" "on" "query"
  "repeatable" "scalar" "schema" "subscription" "type" "union"
] @keyword
