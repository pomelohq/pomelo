; Definitions: the keyword is context, the defined name is the symbol.
(class "class" @context name: (_) @name) @item
(singleton_class "class" @context "<<" @context value: (_) @name) @item
(module "module" @context name: (_) @name) @item
(method "def" @context name: (_) @name) @item
(singleton_method "def" @context object: (_) @context "." @context name: (_) @name) @item
(assignment left: (constant) @name) @item

; Test blocks (RSpec, Minitest): the call says what kind, its first argument names the case.
(call
  method: (identifier) @context
  (#any-of? @context
    "describe" "context" "feature" "scenario" "shared_examples" "shared_context"
    "it" "its" "specify" "example" "test" "focus" "skip" "pending"
    "fdescribe" "fcontext" "fit" "fexample" "xdescribe" "xcontext" "xit" "xexample" "xspecify"
    "it_behaves_like" "it_should_behave_like" "include_context" "include_examples")
  arguments: (argument_list . [(string) (simple_symbol) (constant) (scope_resolution)] @name)) @item

; Rake namespaces and tasks.
(call
  method: (identifier) @context
  (#any-of? @context "namespace" "task")
  arguments: (argument_list . [(string) (simple_symbol)] @name)) @item
