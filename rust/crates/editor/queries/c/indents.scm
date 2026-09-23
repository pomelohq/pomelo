; Statements and expressions that keep going on the next line.
[
  (init_declarator)
  (assignment_expression)
  (call_expression)
  (field_expression)
  (if_statement)
  (for_statement)
  (while_statement)
  (do_statement)
] @indent

(if_statement) @start.if
(else_clause) @start.else
(for_statement) @start.for
(while_statement) @start.while
(do_statement) @start.do
(switch_statement) @start.switch
