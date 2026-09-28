; A body without braces still sits one level in; a brace or `else` on its own line is pulled back
; by the line rules to the statement named here.
(if_statement) @indent @start.if
(else_clause) @start.else
(for_statement) @indent @start.for
(while_statement) @indent @start.while
(do_statement) @indent @start.do
(switch_statement) @start.switch
(case_statement) @indent
