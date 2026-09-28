; Blocks come from the line rules; these name where each block opens so a dedenting keyword can
; line up with the statement it continues.
(if_statement) @start.if
(elif_clause) @start.elif
(else_clause) @start.else
(for_statement) @start.for
(while_statement) @start.while
(try_statement) @start.try
(except_clause) @start.except
(finally_clause) @start.finally
(with_statement) @start.with
(match_statement) @start.match
(case_clause) @start.case
(function_definition) @start.def
(class_definition) @start.class
