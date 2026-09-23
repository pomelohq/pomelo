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
] @indent

(jsx_opening_element ">" @end) @indent

(jsx_element
  (jsx_opening_element) @start
  (jsx_closing_element)? @end) @indent
