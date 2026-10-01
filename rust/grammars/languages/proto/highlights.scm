(comment) @comment

(string) @string
(escape_sequence) @string.escape
(reserved_identifier) @string
(int_lit) @number
(float_lit) @number
[(true) (false)] @boolean

[
  "syntax"
  "edition"
  "package"
  "import"
  "option"
  "message"
  "enum"
  "service"
  "rpc"
  "returns"
  "stream"
  "oneof"
  "map"
  "extend"
  "extensions"
  "group"
  "reserved"
  "to"
  "max"
  "optional"
  "repeated"
  "required"
  "public"
  "weak"
  "export"
  "local"
] @keyword

[(message_name) (enum_name) (service_name) (message_or_enum_type)] @type
(key_type) @type.builtin
(type
  [
    "double" "float" "bool" "string" "bytes"
    "int32" "int64" "uint32" "uint64" "sint32" "sint64"
    "fixed32" "fixed64" "sfixed32" "sfixed64"
  ] @type.builtin)
(rpc_name) @function.method
(package (full_ident) @namespace)
(enum_field (identifier) @constant)
(constant (full_ident) @constant)

(field (identifier) @property)
(map_field (identifier) @property)
(oneof_field (identifier) @property)
(oneof (identifier) @type)
(option (identifier) @property)
(field_option (identifier) @property)
(enum_value_option (identifier) @property)
(block_lit (identifier) @property)

["=" "-" "+"] @operator
[";" "," "." ":"] @punctuation.delimiter
["(" ")" "[" "]" "{" "}" "<" ">"] @punctuation.bracket
