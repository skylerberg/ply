; Highlights for Ply. Scopes follow the common tree-sitter naming so every editor that reads a
; `highlights.scm` picks them up: Neovim, Helix, Zed, and Emacs.

; --- Comments ---------------------------------------------------------------

[
  (line_comment)
  (plain_comment)
] @comment

[
  (item_doc_comment)
  (module_doc_comment)
] @comment.documentation

; --- Keywords ---------------------------------------------------------------

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
  "host"
  "test"
  "let"
  "derive"
  "key"
  "show"
  "gen"
  "numeric"
  "forall"
  "where"
  "returns"
  "fresh"
  "decreases"
  "requires"
  "ensures"
  "cost"
  "bounded"
  "derivable"
  "integer"
  "diverges"
  "read"
  "write"
  "raise"
  "resume"
  "return"
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
  "with_cell"
  "with_hold"
] @keyword.control

"fn" @keyword.function

[
  (boolean)
] @boolean

; --- Items ------------------------------------------------------------------

(function_definition name: (identifier) @function)
(extern_function name: (identifier) @function)

(type_definition name: (identifier) @type)
(effect_definition name: (identifier) @type)
(effect_set_definition name: (identifier) @type)
(law_schema_declaration name: (identifier) @function)

(import_declaration (module_path (identifier) @module))
(import_declaration (identifier) @module)

(module_path (identifier) @module)

; --- Types ------------------------------------------------------------------

(type_application name: (identifier) @type)
(type_application module: (identifier) @module)
(type_arguments (identifier) @type)
(record_type_field name: (identifier) @property)
(sum_variant name: (identifier) @constructor)
; A bare name in a type position is a type; an uppercase one anywhere is a type constructor.
((identifier) @type
  (#match? @type "^[A-Z]"))

; --- Expressions ------------------------------------------------------------

(function_definition parameters: (parameter_list (parameter . (identifier) @parameter)))
(lambda_parameter name: (identifier) @parameter)
(binder name: (identifier) @parameter)
(operation_parameter name: (identifier) @parameter)
(clause_parameters (identifier) @parameter)

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

; --- Literals ---------------------------------------------------------------

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

; --- Operators --------------------------------------------------------------

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

; --- Punctuation ------------------------------------------------------------

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
