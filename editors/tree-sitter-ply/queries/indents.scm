; Indentation for Ply. The language has no layout rule, but `ply fmt` lays every level out at two
; spaces, so a line inside a construct's braces, brackets or parentheses is one level deeper.

; A container indents the lines after its own; two containers that start on one line (a `block`
; and the `function_definition` it is the body of) count once, which the module's own bookkeeping
; does by the line a node starts on.
[
  (function_definition)
  (extern_function)
  (type_definition)
  (effect_definition)
  (effect_set_definition)
  (numeric_declaration)
  (block)
  (record_literal)
  (record_update)
  (list_literal)
  (map_literal)
  (set_literal)
  (tuple_expression)
  (tuple_type)
  (tuple_pattern)
  (argument_list)
  (parameter_list)
  (clause_parameters)
  (operation_parameters)
  (pattern_list)
  (record_type)
  (function_type)
  (type_arguments)
  (generic_parameters)
  (row)
  (effect_set)
  (match_expression)
  (handle_expression)
  (parallel_expression)
  (with_binder_body)
] @indent.begin

; A closer ends the level it opened; it is the branch that re-indents the closer's own line.
[
  "}"
  "]"
  ")"
  (type_gt)
] @indent.end

[
  "}"
  "]"
  ")"
  (type_gt)
] @indent.branch
