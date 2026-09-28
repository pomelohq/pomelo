(switch_body
  [(switch_case ":" @start) (switch_default ":" @start)]
  .
  [(switch_case) (switch_default) "}"] @end)
(member_expression) @indent
(variable_declarator) @indent

(jsx_element (jsx_opening_element) @start (jsx_closing_element) @end) @indent
(jsx_opening_element ">" @end) @indent
(jsx_self_closing_element "/>" @end) @indent
