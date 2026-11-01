[
  (line_comment)
  (plain_comment)
] @comment

[
  (item_doc_comment)
  (module_doc_comment)
] @comment.documentation

[
  "import"
  "as"
  "pub"
  "extern"
  "const"
  "reuse"
  "transparent"
  "opaque"
  "new"
  "type"
  "effect"
  "nondet"
  "set"
  "law"
  "schema"
  "test"
  "let"
  "derive"
  "key"
  "show"
  "invariant"
  "release"
  "numeric"
  "host"
  "once"
  "forall"
  "where"
  "returns"
  "fresh"
  "decreases"
  "requires"
  "ensures"
  "cost"
  "space"
  "bounded"
  "derivable"
  "integer"
  "diverges"
  "read"
  "write"
  "raise"
  "resume"
  "return"
  "var"
  "for"
  "in"
  "by"
] @keyword

[
  "if"
  "else"
  "match"
  "handle"
  "with"
  "try"
  "simulate"
  "parallel"
  "with_scope"
] @keyword.control

"fn" @keyword.function
(keep_marker) @keyword

[
  (boolean)
] @boolean

(function_definition name: (identifier) @function)
(const_declaration name: (identifier) @constant)
(extern_function name: (identifier) @function)

(type_definition name: (identifier) @type)
(extern_type name: (identifier) @type)
(effect_definition name: (identifier) @type)
(effect_set_definition name: (identifier) @type)
(law_schema_declaration name: (identifier) @function)

(import_declaration (module_path (identifier) @module))
(import_declaration (identifier) @module)

(module_path (identifier) @module)

(type_application name: (identifier) @type)
(type_application module: (identifier) @module)
(type_arguments (identifier) @type)
(record_type_field name: (identifier) @property)
(sum_variant name: (identifier) @constructor)
((identifier) @type
  (#match? @type "^[A-Z]"))

(function_definition parameters: (parameter_list (parameter . (identifier) @parameter)))
(lambda_parameter name: (identifier) @parameter)
(binder name: (identifier) @parameter)
(operation_parameter name: (identifier) @parameter)
(clause_parameters (identifier) @parameter)

(law_declaration condition: (call_expression function: (identifier) @keyword (#eq? @keyword "identity")))
(law_schema_declaration condition: (call_expression function: (identifier) @keyword (#eq? @keyword "identity")))
(call_expression function: (identifier) @function.call)
(call_expression function: (qualified_name (identifier) @function.call))
(call_expression function: (field_expression field: (identifier) @function.method))

(constructor_pattern name: (identifier) @constructor)
(pattern_list (identifier) @variable)

(field_expression field: (identifier) @property)
(record_field_value . (identifier) @property)
(record_field_punned name: (identifier) @property)
(record_pattern_field name: (identifier) @property)
(tuple_field) @property

(named_argument name: (identifier) @parameter)

(operation (identifier) @function.method)
(operation_name operation: (identifier) @function.method)
(operation_name effect: (identifier) @module)

(resource_label (identifier) @variable.parameter)
(wildcard_label (identifier) @variable.parameter)
(label_argument (identifier) @variable.parameter)

(wildcard) @variable.builtin

(number) @number
(char_literal) @character
(byte_literal) @character

(string) @string
(bytes_string) @string
(interpolated_string) @string
(tagged_string) @string
(line_string) @string

(escape_sequence) @string.escape
(interpolation) @punctuation.special
(tagged_string tag: (identifier) @function.macro)

(label) @label

[
  "||"
  "&&"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "|"
  "^"
  "&"
  "<<"
  ">>"
  ">>>"
  "++"
  "+"
  "-"
  "+%"
  "-%"
  "+|"
  "-|"
  "*"
  "/"
  "%"
  "*%"
  "*|"
  "!"
  "~"
  "?"
  "="
  "->"
  ".."
  "..="
  ":"
  "::"
  "."
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  (type_gt)
] @punctuation.bracket

[
  ","
  ";"
] @punctuation.delimiter

(hole_expression) @keyword
