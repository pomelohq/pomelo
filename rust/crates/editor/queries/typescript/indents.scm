(switch_body
  [(switch_case ":" @start) (switch_default ":" @start)]
  .
  [(switch_case) (switch_default) "}"] @end)
(member_expression) @indent
(variable_declarator) @indent
(type_parameters ">" @end) @indent
(type_arguments ">" @end) @indent
