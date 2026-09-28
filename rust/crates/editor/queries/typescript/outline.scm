(class_declaration "class" @context name: (_) @name) @item

(method_definition "static"? @context "async"? @context ["get" "set"]? @context name: (_) @name) @item

(function_declaration "async"? @context "function" @context name: (_) @name) @item

(generator_function_declaration "async"? @context "function" @context "*" @context name: (_) @name) @item

(program
  (lexical_declaration ["let" "const"] @context (variable_declarator name: (identifier) @name) @item))

(program
  (export_statement
    (lexical_declaration ["let" "const"] @context (variable_declarator name: (identifier) @name) @item)))

(abstract_class_declaration "abstract" @context "class" @context name: (_) @name) @item

(interface_declaration "interface" @context name: (_) @name) @item

(type_alias_declaration "type" @context name: (_) @name) @item

(enum_declaration "enum" @context name: (_) @name) @item

(internal_module "namespace" @context name: (_) @name) @item

(public_field_definition name: (_) @name) @item

(method_signature name: (_) @name) @item

(property_signature name: (_) @name) @item
