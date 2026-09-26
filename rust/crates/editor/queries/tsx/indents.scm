; Statements and expressions that keep going on the next line.
[
  (lexical_declaration)
  (variable_declaration)
  (assignment_expression)
  (call_expression)
  (member_expression)
  (if_statement)
  (for_statement)
  (while_statement)
  (type_alias_declaration)
] @indent

(_ "<" ">" @end) @indent

(jsx_opening_element ">" @end) @indent

(jsx_element
  (jsx_opening_element) @start
  (jsx_closing_element)? @end) @indent
