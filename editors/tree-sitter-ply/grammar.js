// Tree-sitter grammar for Ply. The language is defined by `crates/ply-compiler/ply`; this grammar
// follows the lexer and the parser there, and parses every `.ply` file in the tree without error.
//
// Reserved words (from `lexer.ply`'s `is_keyword`): pub import fn type effect nondet test let if
// else match handle with true false. Every other keyword the language uses is contextual, and this
// grammar accepts it only where the language does.

const PREC = {
  or: 1,
  and: 2,
  comparison: 3,
  bitor: 4,
  bitxor: 5,
  bitand: 6,
  shift: 7,
  concat: 8,
  additive: 9,
  multiplicative: 10,
  unary: 11,
  try: 12,
  call: 13,
  field: 14,
};

const RESERVED = ['fn', 'if', 'pub', 'let', 'type', 'test', 'else', 'with', 'true', 'false', 'match',
  'import', 'effect', 'nondet', 'handle'];

// A field name may be any reserved word (`{nondet: Bool}`), but a punned field may not.
const FIELD_NAME = $ => choice($.identifier, ...RESERVED);

module.exports = grammar({
  name: 'ply',

  word: $ => $.identifier,

  // `>` closing a type argument is one of the adjacent `>`s the lexer keeps apart, so a scanner
  // emits them one at a time while `>>` and `>>>` stay expression tokens.
  externals: $ => [$.type_gt, $.label],

  extras: $ => [
    /[ \t\r\n\f\v]/,
    $.line_comment,
    $.module_doc_comment,
    $.item_doc_comment,
    $.plain_comment,
  ],

  conflicts: $ => [
    [$.sum_variant, $._type],
    [$.function_type, $.tuple_type],
  ],



  rules: {
    source_file: $ => repeat(choice($.import_declaration, $._item)),

    // --- Imports ------------------------------------------------------------

    import_declaration: $ => seq(
      'import',
      $.module_path,
      optional(choice(
        seq('as', $.identifier),
        seq('(', commaSep1($.identifier), ')'),
      )),
    ),

    module_path: $ => seq($.identifier, repeat(seq('.', $.identifier))),

    // --- Items --------------------------------------------------------------

    _item: $ => choice(
      $.function_definition,
      $.extern_function,
      $.type_definition,
      $.effect_definition,
      $.effect_set_definition,
      $.test_declaration,
      $.law_declaration,
      $.law_schema_declaration,
      $.derive_declaration,
      $.key_declaration,
      $.show_declaration,
      $.gen_declaration,
      $.numeric_declaration,
    ),

    function_definition: $ => seq(
      optional('pub'),
      optional(choice(
        seq(optional('transparent'), optional('reuse')),
        'const',
      )),
      'fn',
      field('name', $.identifier),
      optional($.generic_parameters),
      field('parameters', $.parameter_list),
      optional(seq('->', field('return_type', $._type), optional($.effect_row))),
      repeat($._spec_clause),
      optional(choice(
        seq('=', field('body', $._expression)),
        field('body', $.block),
      )),
    ),

    extern_function: $ => seq(
      'extern', 'fn',
      field('name', $.identifier),
      optional($.generic_parameters),
      field('parameters', $.parameter_list),
      optional(seq('->', field('return_type', $._type))),
      optional($.effect_row),
      repeat($._spec_clause),
    ),

    _spec_clause: $ => choice(
      $.where_clause,
      $.returns_clause,
      $.decreases_clause,
      $.requires_clause,
      $.ensures_clause,
      $.cost_clause,
    ),

    where_clause: $ => seq('where', seq($.predicate, repeat(seq(',', $.predicate)))),
    predicate: $ => choice(
      seq(
        'derivable',
        '(',
        field('property', $.identifier),
        ',',
        field('type', $._type),
        ')',
      ),
      seq(
        field('property', choice('numeric', 'integer')),
        '(',
        field('type', $._type),
        ')',
      ),
    ),
    returns_clause: $ => seq('returns', choice('fresh', $.identifier)),
    decreases_clause: $ => seq('decreases', field('measure', $._expression)),
    requires_clause: $ => seq('requires', field('condition', $._expression)),
    ensures_clause: $ => seq('ensures', field('condition', $._expression)),
    cost_clause: $ => seq('cost', field('measure', $._expression)),

    parameter_list: $ => seq('(', optional(commaSep1($.parameter)), ')'),
    parameter: $ => seq(
      field('name', choice($.identifier, $.wildcard)),
      ':',
      field('type', $._type),
      optional(seq('=', field('default', $._expression))),
    ),

    generic_parameters: $ => choice(
      seq('<', $.type_gt),
      seq('<', commaSep1($._generic_parameter), optional(seq('|', optional(commaSep1($.row_parameter)))), $.type_gt),
      seq('<', '|', commaSep1($.row_parameter), $.type_gt),
    ),
    _generic_parameter: $ => choice($.type_parameter, $.label_parameter),
    type_parameter: $ => $.identifier,
    label_parameter: $ => seq('[', commaSep1($.identifier), ']'),
    row_parameter: $ => $.identifier,

    // --- Types --------------------------------------------------------------

    type_definition: $ => seq(
      optional('pub'),
      optional('opaque'),
      'type',
      field('name', $.identifier),
      optional($.generic_parameters),
      '=',
      choice($.new_record_body, $.sum_type_body, $.alias_type),
    ),

    new_record_body: $ => seq('new', $.record_type),

    sum_type_body: $ => seq(
      optional('|'),
      sep1('|', $.sum_variant),
    ),
    sum_variant: $ => seq(
      field('name', $.identifier),
      optional(seq('(', optional(commaSep1(field('variant', $._type))), ')')),
    ),

    alias_type: $ => prec(1, $._type),

    _type: $ => choice(
      $.function_type,
      $.record_type,
      $.tuple_type,
      $.type_application,
      $.identifier,
      $.parenthesized_type,
      $.unit_type,
    ),

    // `List<Int>`, `Map<k, v>`, `fs::MemFs`, `bin::BinCodec<EmbedRead>`; a bare name is a
    // `type_variable` in a signature and a `type_constructor` in a body, which the checker tells
    // apart by case, so one token serves both and a query reads the case.
    type_application: $ => choice(
      seq(
        optional(seq(field('module', $.identifier), '::')),
        field('name', $.identifier),
        $.type_arguments,
      ),
      seq(field('module', $.identifier), '::', field('name', $.identifier)),
    ),

    type_arguments: $ => choice(
      seq('<', commaSep1($._type_argument), optional(seq('|', optional(commaSep1($.row)))), $.type_gt),
      seq('<', '|', commaSep1($.row), $.type_gt),
    ),
    _type_argument: $ => choice($._type, $.label_argument),
    label_argument: $ => seq('[', $.identifier, ']'),

    function_type: $ => prec.right(1, seq(
      optional($.generic_parameters),
      '(',
      optional(commaSep1(field('parameter', $._type))),
      ')',
      '->',
      field('return_type', $._type),
      optional($.effect_row),
      optional($.cost_clause),
    )),

    tuple_type: $ => seq('(', $._type, ',', commaSep1($._type), ')'),
    parenthesized_type: $ => seq('(', $._type, ')'),
    unit_type: $ => seq('(', ')'),

    record_type: $ => seq('{', commaSep1($.record_type_field), '}'),
    record_type_field: $ => seq(field('name', FIELD_NAME($)), ':', field('type', $._type)),

    // --- Effects ------------------------------------------------------------

    effect_definition: $ => seq(
      optional('pub'),
      optional('nondet'),
      'effect',
      field('name', $.identifier),
      '{',
      repeat(seq($.operation, optional(','))),
      '}',
    ),

    operation: $ => seq(
      field('mode', choice('read', 'write', 'raise')),
      field('name', $.identifier),
      optional($.resource_label),
      optional($.type_parameters),
      $.operation_parameters,
      optional(seq('->', field('return_type', $._type))),
    ),
    operation_parameters: $ => seq('(', optional(commaSep1($.operation_parameter)), ')'),
    operation_parameter: $ => choice(
      seq(field('name', $.identifier), ':', field('type', $._type)),
      field('type', $._type),
    ),

    type_parameters: $ => seq('<', commaSep1($.identifier), '>'),

    effect_set_definition: $ => seq(
      optional('pub'),
      'effect', 'set',
      field('name', $.identifier),
      '=',
      $.effect_set,
    ),
    effect_set: $ => seq('{', commaSep1($._effect_set_entry), '}'),
    _effect_set_entry: $ => choice($.atom, $.qualified_name, $.identifier),

    // An atom is `effect.mode[resource]`, or `effect.mode` for a singleton.
    atom: $ => choice(
      'diverges',
      seq(
        field('effect', choice($.qualified_name, $.identifier)),
        '.',
        field('mode', choice('read', 'write', $.identifier)),
        optional($.resource_label),
      ),
    ),

    effect_row: $ => seq('/', choice($.row, seq($.row, 'bounded'))),
    row: $ => choice(
      seq('{', optional($._row_body), '}'),
      $.row_variable,
    ),
    _row_body: $ => seq(
      commaSep1($.row_atom),
      optional(seq('|', choice('diverges', $.row_variable), optional('bounded'))),
    ),
    row_atom: $ => seq(choice($.atom, $.qualified_name, $.identifier), optional('bounded')),
    row_variable: $ => $.identifier,

    // --- Tests and laws -----------------------------------------------------

    test_declaration: $ => seq(
      'test',
      optional(seq('/', 'nondet')),
      field('label', $.label),
      optional(seq(
        'for',
        field('case', $.identifier),
        ':',
        field('case_type', $._type),
        'in',
        field('table', $._expression),
      )),
      field('body', $.block),
    ),

    law_declaration: $ => seq(
      'law',
      optional(seq('/', 'host')),
      field('label', $.label),
      optional(choice(
        seq('=', field('instantiation', $._expression)),
        seq(
          optional(seq('forall', '(', commaSep1($.binder), ')')),
          optional(seq('where', field('guard', $._expression))),
          optional($.cost_clause),
          field('body', $.block),
        ),
      )),
    ),

    law_schema_declaration: $ => seq(
      optional('pub'),
      'law', 'schema',
      field('name', $.identifier),
      optional($.generic_parameters),
      $.parameter_list,
      optional(seq('forall', '(', commaSep1($.binder), ')')),
      optional(seq('where', field('guard', $._expression))),
      optional($.cost_clause),
      field('body', $.block),
    ),

    binder: $ => seq(field('name', $.identifier), ':', field('type', $._type)),

    derive_declaration: $ => seq(
      optional('pub'),
      'derive',
      field('deriver', $.identifier),
      'for',
      field('type', $._type),
    ),

    key_declaration: $ => seq(optional('pub'), 'key', 'for', field('type', $._type), 'by', field('function', $.identifier)),
    show_declaration: $ => seq(optional('pub'), 'show', 'for', field('type', $._type), 'by', field('function', $.identifier)),
    gen_declaration: $ => seq(optional('pub'), 'gen', 'for', field('type', $._type), 'by', field('function', $.identifier)),
    numeric_declaration: $ => seq(
      optional('pub'),
      'numeric',
      'for',
      field('type', $._type),
      'by',
      '{',
      commaSep1($.numeric_operation),
      '}',
    ),
    numeric_operation: $ => seq(
      field('operation', choice('add', 'sub', 'mul', 'neg', 'of_int')),
      ':',
      field('function', $.identifier),
    ),

    // --- Expressions --------------------------------------------------------

    _expression: $ => choice(
      $._literal,
      $.identifier,
      $.qualified_name,
      $.field_expression,
      $.call_expression,
      $.try_expression,
      $.unary_expression,
      $._binary_expression,
      $.if_expression,
      $.match_expression,
      $.lambda_expression,
      $.block,
      $.record_literal,
      $.record_update,
      $.list_literal,
      $.map_literal,
      $.set_literal,
      $.tuple_expression,
      $.parenthesized_expression,
      $.handle_expression,
      $.try_block,
      $.with_cell_expression,
      $.with_hold_expression,
      $.simulate_expression,
      $.parallel_expression,
    ),

    qualified_name: $ => seq($.identifier, '::', $.identifier),

    field_expression: $ => prec(PREC.field, seq(
      field('base', $._expression),
      '.',
      field('field', choice($.identifier, $.tuple_field)),
    )),
    tuple_field: $ => token(/_[0-9]+/),

    call_expression: $ => prec(PREC.call, seq(
      field('function', $._expression),
      optional($.resource_label),
      field('arguments', $.argument_list),
    )),
    argument_list: $ => seq('(', optional(commaSep1($._argument)), ')'),
    _argument: $ => choice($.named_argument, $._expression),
    named_argument: $ => seq(field('name', $.identifier), ':', field('value', $._expression)),

    resource_label: $ => seq('[', commaSep1(choice($.identifier, $.wildcard_label)), ']'),
    wildcard_label: $ => seq('*', optional($.identifier)),

    try_expression: $ => prec(PREC.try, seq($._expression, '?')),

    unary_expression: $ => prec(PREC.unary, seq(
      field('operator', choice('-', '!', '~')),
      field('operand', $._expression),
    )),

    _binary_expression: $ => choice(
      $._or_expression,
      $._and_expression,
      $._comparison_expression,
      $._bitor_expression,
      $._bitxor_expression,
      $._bitand_expression,
      $._shift_expression,
      $._concat_expression,
      $._additive_expression,
      $._multiplicative_expression,
    ),
    _or_expression: $ => prec.left(PREC.or, seq($._expression, '||', $._expression)),
    _and_expression: $ => prec.left(PREC.and, seq($._expression, '&&', $._expression)),
    _comparison_expression: $ => prec.left(PREC.comparison, seq(
      $._expression,
      choice('==', '!=', '<', '<=', '>', '>='),
      $._expression,
    )),
    _bitor_expression: $ => prec.left(PREC.bitor, seq($._expression, '|', $._expression)),
    _bitxor_expression: $ => prec.left(PREC.bitxor, seq($._expression, '^', $._expression)),
    _bitand_expression: $ => prec.left(PREC.bitand, seq($._expression, '&', $._expression)),
    // `>>` and `>>>` are two and three `>` tokens: a type argument closes with adjacent `>`s.
    _shift_expression: $ => prec.left(PREC.shift, seq(
      $._expression,
      choice('<<', '>>', '>>>'),
      $._expression,
    )),
    _concat_expression: $ => prec.left(PREC.concat, seq($._expression, '++', $._expression)),
    _additive_expression: $ => prec.left(PREC.additive, seq(
      $._expression,
      choice('+', '-', '+%', '-%', '+|', '-|'),
      $._expression,
    )),
    _multiplicative_expression: $ => prec.left(PREC.multiplicative, seq(
      $._expression,
      choice('*', '/', '%', '*%', '*|'),
      $._expression,
    )),

    if_expression: $ => prec.right(1, seq(
      'if',
      field('condition', $._expression),
      field('then', $.block),
      optional(seq('else', field('otherwise', choice($.if_expression, $.block)))),
    )),

    match_expression: $ => seq(
      'match',
      field('value', $._expression),
      '{',
      commaSep1($.match_arm),
      '}',
    ),
    match_arm: $ => seq(
      field('pattern', $._pattern),
      optional(seq('if', field('guard', $._expression))),
      '->',
      field('body', $._expression),
    ),

    lambda_expression: $ => prec.right(seq(
      choice(
        seq('|', commaSep1($.lambda_parameter), '|'),
        '||',
      ),
      optional(seq('->', field('return_type', $._type))),
      field('body', $._expression),
    )),
    lambda_parameter: $ => seq(
      choice(field('name', $.identifier), '_'),
      optional(seq(':', field('type', $._type))),
    ),

    block: $ => prec.dynamic(2, prec(2, seq('{', repeat($._statement), '}'))),
    _statement: $ => choice(
      $.let_statement,
      $.expression_statement,
    ),
    let_statement: $ => seq(
      'let',
      field('pattern', $._pattern),
      optional(seq(':', field('type', $._type))),
      '=',
      field('value', $._expression),
      optional(seq('else', field('otherwise', $.block))),
      ';',
    ),
    expression_statement: $ => seq($._expression, optional(';')),

    // `{x}` is a block whose value is `x`; a record needs a named field or, punned, a comma.
    record_literal: $ => prec(1, seq('{', choice(
      seq($.record_field_value, repeat(seq(',', $.record_field)), optional(',')),
      seq($.record_field_punned, repeat1(seq(',', $.record_field)), optional(',')),
    ), '}')),
    record_update: $ => seq(
      '{',
      '..',
      field('base', $._expression),
      optional(seq(',', commaSep1($.record_field))),
      '}',
    ),
    record_field: $ => choice($.record_field_value, $.record_field_punned),
    record_field_value: $ => seq(
      field('name', choice($.identifier, $._keyword, $.tuple_field)),
      ':',
      field('value', $._expression),
    ),
    record_field_punned: $ => field('name', choice($.identifier, $.tuple_field)),

    list_literal: $ => seq('[', optional(commaSep1($._expression)), ']'),
    map_literal: $ => seq('#{', optional(commaSep1($.map_entry)), '}'),
    map_entry: $ => seq(field('key', $._expression), ':', field('value', $._expression)),
    set_literal: $ => seq('#[', optional(commaSep1($._expression)), ']'),

    tuple_expression: $ => seq('(', $._expression, ',', commaSep1($._expression), ')'),
    parenthesized_expression: $ => seq('(', $._expression, ')'),

    handle_expression: $ => seq(
      'handle',
      field('body', $._expression),
      'with',
      '{',
      commaSep1($.handler_clause),
      '}',
    ),
    handler_clause: $ => choice($.return_clause, $.operation_clause),
    return_clause: $ => seq('return', field('binder', $.identifier), '->', field('body', $._expression)),
    operation_clause: $ => seq(
      field('operation', $.operation_name),
      optional($.resource_label),
      field('parameters', $.clause_parameters),
      optional(seq('resume', field('continuation', $.identifier))),
      '->',
      field('body', $._expression),
    ),
    clause_parameters: $ => seq('(', optional(commaSep1(choice($.identifier, $.wildcard))), ')'),
    operation_name: $ => seq(
      field('effect', choice($.qualified_name, $.identifier)),
      '.',
      field('operation', $.identifier),
    ),

    try_block: $ => seq('try', optional($.raise_name), field('body', $.block)),
    raise_name: $ => seq('[', $.operation_name, ']'),

    with_cell_expression: $ => seq(
      'with_cell',
      field('label', $.resource_label),
      '(',
      commaSep1(field('initial', $._expression)),
      ')',
      field('body', $.with_binder_body),
    ),
    with_hold_expression: $ => seq(
      'with_hold',
      field('label', $.resource_label),
      '(',
      commaSep1(field('initial', $._expression)),
      ')',
      field('body', $.with_binder_body),
    ),
    with_binder_body: $ => seq('{', field('binder', $.identifier), '->', field('body', $._expression), '}'),

    simulate_expression: $ => seq('simulate', field('body', $.block)),
    parallel_expression: $ => seq(
      'parallel',
      '{',
      commaSep1($._expression),
      '}',
    ),

    // --- Patterns -----------------------------------------------------------

    _pattern: $ => choice(
      $.wildcard,
      $.identifier,
      $.literal_pattern,
      $.range_pattern,
      $.constructor_pattern,
      $.list_pattern,
      $.record_pattern,
      $.tuple_pattern,
      $.parenthesized_pattern,
      $.or_pattern,
    ),

    wildcard: $ => '_',

    literal_pattern: $ => choice(
      $.number,
      $.string,
      $.bytes_string,
      $.char_literal,
      $.byte_literal,
      'true',
      'false',
      $.unit,
      seq('-', $.number),
    ),

    range_pattern: $ => seq($.literal_pattern, '..=', $.literal_pattern),

    constructor_pattern: $ => choice(
      seq(field('module', $.identifier), '::', field('name', $.identifier), optional($.pattern_list)),
      seq(field('name', $.identifier), $.pattern_list),
    ),
    pattern_list: $ => seq('(', optional(commaSep1($._pattern)), ')'),

    list_pattern: $ => seq(
      '[',
      optional(choice(
        seq($._pattern, repeat(seq(',', $._pattern)), ',', '..', optional(field('rest', $.identifier))),
        commaSep1($._pattern),
      )),
      optional(','),
      ']',
    ),

    record_pattern: $ => seq(
      '{',
      optional(choice(
        seq($.record_pattern_field, repeat(seq(',', $.record_pattern_field)), ',', '..'),
        commaSep1($.record_pattern_field),
        '..',
      )),
      optional(','),
      '}',
    ),
    record_pattern_field: $ => choice(
      seq(field('name', $.identifier), optional(seq(':', field('pattern', $._pattern)))),
      seq(field('name', $._keyword), ':', field('pattern', $._pattern)),
    ),

    tuple_pattern: $ => seq('(', $._pattern, ',', commaSep1($._pattern), ')'),
    parenthesized_pattern: $ => seq('(', $._pattern, ')'),

    or_pattern: $ => prec.left(seq($._pattern, '|', $._pattern)),

    // --- Literals -----------------------------------------------------------

    _literal: $ => choice(
      $.number,
      $.boolean,
      $.string,
      $.bytes_string,
      $.char_literal,
      $.byte_literal,
      $.interpolated_string,
      $.tagged_string,
      $.line_string,
      $.unit,
    ),

    boolean: $ => choice('true', 'false'),
    unit: $ => seq('(', ')'),

    _keyword: $ => choice(...RESERVED),

    number: $ => token(choice(
      /0[xX][0-9a-fA-F][0-9a-fA-F_]*(u8|u16|u32|u64|u128|i8|i16|i32|i64|i128)?/,
      /[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9][0-9_]*)?(m|u8|u16|u32|u64|u128|i8|i16|i32|i64|i128)?/,
    )),

    string: $ => seq(
      '"',
      repeat(choice($.escape_sequence, $.string_content)),
      '"',
    ),
    string_content: $ => token.immediate(prec(10, /[^"\\\n]+/)),
    escape_sequence: $ => token.immediate(/\\(u\{[0-9a-fA-F]{1,6}\}|x[0-9a-fA-F]{2}|[0nrt'"\\])/),

    bytes_string: $ => seq(
      $.bytes_open,
      repeat(choice($.escape_sequence, $.bytes_content)),
      '"',
    ),
    bytes_open: $ => token('b"'),
    bytes_content: $ => token.immediate(prec(10, /[^"\\\n]+/)),

    interpolated_string: $ => seq(
      $.interpolation_open,
      repeat(choice($.interpolation, $.escape_sequence, $.interpolated_content)),
      '"',
    ),
    interpolation_open: $ => token('f"'),
    interpolated_content: $ => token.immediate(prec(10, /([^"\\{}\n]|\{\{|\}\})+/)),
    interpolation: $ => seq('{', field('value', $._expression), '}'),

    tagged_string: $ => seq(
      field('tag', $.identifier),
      $.tag_open,
      repeat(choice($.interpolation, $.escape_sequence, $.interpolated_content)),
      '"',
    ),
    tag_open: $ => token.immediate('"'),

    char_literal: $ => token(/'(\\.|[^'\\\n])*'/),
    byte_literal: $ => token(/b'(\\.|[^'\\\n])*'/),

    line_string: $ => token(/(\\\\[^\n]*)(\n[ \t]*\\\\[^\n]*)*/),

    // --- Comments -----------------------------------------------------------

    line_comment: $ => token(/\/\/[^\n]*/),
    module_doc_comment: $ => token(prec(3, /\/\/![^\n]*/)),
    item_doc_comment: $ => token(prec(2, /\/\/\/[^\n]*/)),
    plain_comment: $ => token(prec(4, /\/\/\/\/+[^\n]*/)),

    identifier: $ => /[a-zA-Z_][a-zA-Z0-9_]*/,
  },
});

function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)), optional(','));
}

function sep1(separator, rule) {
  return seq(rule, repeat(seq(separator, rule)));
}
