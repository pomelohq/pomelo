(class_declaration "class" @context name: (_) @name) @item

(method_definition "static"? @context "async"? @context ["get" "set"]? @context name: (_) @name) @item

(function_declaration "async"? @context "function" @context name: (_) @name) @item

(generator_function_declaration "async"? @context "function" @context "*" @context name: (_) @name) @item

(program
  (lexical_declaration ["let" "const"] @context (variable_declarator name: (identifier) @name) @item))

(program
  (export_statement
    (lexical_declaration ["let" "const"] @context (variable_declarator name: (identifier) @name) @item)))

(field_definition property: (_) @name) @item
