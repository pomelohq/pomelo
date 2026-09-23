(struct_specifier "struct" @context name: (_) @name body: (_)) @item

(union_specifier "union" @context name: (_) @name body: (_)) @item

(enum_specifier "enum" @context name: (_) @name body: (_)) @item

(enumerator name: (_) @name) @item

(type_definition "typedef" @context declarator: (_) @name) @item

(function_definition declarator: (function_declarator declarator: (_) @name)) @item

(function_definition declarator: (pointer_declarator declarator: (function_declarator declarator: (_) @name))) @item

(declaration declarator: (function_declarator declarator: (_) @name)) @item

(class_specifier "class" @context name: (_) @name body: (_)) @item

(namespace_definition "namespace" @context name: (_) @name) @item

(field_declaration declarator: (function_declarator declarator: (_) @name)) @item
