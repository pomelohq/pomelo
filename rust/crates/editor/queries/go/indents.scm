; Cases sit level with their switch: the first case cuts the brace block short, and each case
; indents from its colon up to the next case or the closing brace, so an empty case indents too.
(expression_switch_statement "{" . [(expression_case) (default_case)] @outdent)
(type_switch_statement "{" . [(type_case) (default_case)] @outdent)
(select_statement "{" . [(communication_case) (default_case)] @outdent)

(expression_switch_statement
  [(expression_case ":" @start) (default_case ":" @start)]
  .
  [(expression_case) (default_case) "}"] @end)
(type_switch_statement
  [(type_case ":" @start) (default_case ":" @start)]
  .
  [(type_case) (default_case) "}"] @end)
(select_statement
  [(communication_case ":" @start) (default_case ":" @start)]
  .
  [(communication_case) (default_case) "}"] @end)
