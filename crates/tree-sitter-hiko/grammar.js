const PREC = {
  annotation: 1,
  orelse: 2,
  andalso: 3,
  compare: 4,
  cons: 5,
  add: 6,
  mul: 7,
  application: 8,
  unary: 9,
  type_arrow: 1,
  type_tuple: 2,
  type_application: 3,
};

module.exports = grammar({
  name: "hiko",

  extras: $ => [
    /\s+/,
    $.comment,
  ],

  word: $ => $.identifier,

  supertypes: $ => [
    $._declaration,
    $._expression,
    $._atomic_expression,
    $._pattern,
    $._type_expression,
  ],

  rules: {
    source_file: $ => repeat($._declaration),

    comment: _ => token(seq(
      "(*",
      repeat(choice(
        /[^*]+/,
        /\*+[^*)]/,
      )),
      /\*+\)/,
    )),

    _declaration: $ => choice(
      $.value_declaration,
      $.value_rec_declaration,
      $.function_declaration,
      $.datatype_declaration,
      $.type_alias_declaration,
      $.local_declaration,
      $.use_declaration,
      $.import_declaration,
      $.signature_declaration,
      $.structure_declaration,
      $.effect_declaration,
    ),

    value_declaration: $ => seq(
      "val",
      field("pattern", $._pattern),
      "=",
      field("value", $._expression),
    ),

    value_rec_declaration: $ => seq(
      "val",
      "rec",
      field("name", $.identifier),
      "=",
      field("value", $.fn_expression),
    ),

    function_declaration: $ => seq(
      "fun",
      $.function_binding,
      repeat(seq("and", $.function_binding)),
    ),

    function_binding: $ => seq(
      field("name", $.identifier),
      $.function_clause,
      repeat(seq("|", field("name", $.identifier), $.function_clause)),
    ),

    function_clause: $ => seq(
      repeat1(field("parameter", $.atomic_pattern)),
      "=",
      field("body", $._expression),
    ),

    datatype_declaration: $ => seq(
      "datatype",
      optional(field("type_parameters", $.type_parameters)),
      field("name", $.name),
      "=",
      optional("|"),
      $.constructor_declaration,
      repeat(seq("|", $.constructor_declaration)),
    ),

    constructor_declaration: $ => seq(
      field("name", $.upper_identifier),
      optional(seq("of", field("payload", $._type_expression))),
    ),

    type_alias_declaration: $ => seq(
      "type",
      optional(field("type_parameters", $.type_parameters)),
      field("name", $.name),
      "=",
      field("value", $._type_expression),
    ),

    local_declaration: $ => seq(
      "local",
      repeat($._declaration),
      "in",
      repeat($._declaration),
      "end",
    ),

    use_declaration: $ => seq(
      "use",
      field("path", $.string_literal),
    ),

    import_declaration: $ => seq(
      "import",
      field("package", $._import_package_name),
      ".",
      field("module", $.upper_identifier),
    ),

    signature_declaration: $ => seq(
      "signature",
      field("name", $.upper_identifier),
      "=",
      "sig",
      repeat($.signature_specification),
      "end",
    ),

    signature_specification: $ => choice(
      seq(
        "type",
        optional(field("type_parameters", $.type_parameters)),
        field("name", $.name),
      ),
      seq(
        "val",
        field("name", $.identifier),
        ":",
        field("type", $._type_expression),
      ),
    ),

    structure_declaration: $ => seq(
      "structure",
      field("name", $.upper_identifier),
      optional(seq(field("ascription_operator", choice(":", ":>")), field("signature", $.upper_identifier))),
      "=",
      "struct",
      repeat($._declaration),
      "end",
    ),

    effect_declaration: $ => seq(
      "effect",
      field("name", $.upper_identifier),
      optional(seq("of", field("payload", $._type_expression))),
    ),

    _expression: $ => $.annotated_expression,

    annotated_expression: $ => choice(
      prec.right(PREC.annotation, seq(
        field("expression", $.orelse_expression),
        ":",
        field("type", $._type_expression),
      )),
      $.orelse_expression,
    ),

    orelse_expression: $ => choice(
      prec.right(PREC.orelse, seq(
        field("left", $.andalso_expression),
        "orelse",
        field("right", $.orelse_expression),
      )),
      $.andalso_expression,
    ),

    andalso_expression: $ => choice(
      prec.right(PREC.andalso, seq(
        field("left", $.comparison_expression),
        "andalso",
        field("right", $.andalso_expression),
      )),
      $.comparison_expression,
    ),

    comparison_expression: $ => choice(
      prec.left(PREC.compare, seq(
        field("left", $.cons_expression),
        field("operator", $.comparison_operator),
        field("right", $.cons_expression),
      )),
      $.cons_expression,
    ),

    comparison_operator: _ => choice(
      "=",
      "<>",
      "<",
      ">",
      "<=",
      ">=",
      "<.",
      ">.",
      "<=.",
      ">=.",
    ),

    cons_expression: $ => choice(
      prec.right(PREC.cons, seq(
        field("head", $.additive_expression),
        "::",
        field("tail", $.cons_expression),
      )),
      $.additive_expression,
    ),

    additive_expression: $ => choice(
      prec.left(PREC.add, seq(
        field("left", $.additive_expression),
        field("operator", $.additive_operator),
        field("right", $.multiplicative_expression),
      )),
      $.multiplicative_expression,
    ),

    additive_operator: _ => choice("+", "-", "+.", "-.", "^"),

    multiplicative_expression: $ => choice(
      prec.left(PREC.mul, seq(
        field("left", $.multiplicative_expression),
        field("operator", $.multiplicative_operator),
        field("right", $.application_expression),
      )),
      $.application_expression,
    ),

    multiplicative_operator: _ => choice("*", "/", "mod", "*.", "/."),

    application_expression: $ => choice(
      prec.left(PREC.application, seq(
        field("function", $.application_expression),
        field("argument", $.application_argument),
      )),
      prec.left(PREC.application, seq(
        field("function", $.unary_expression),
        field("argument", $.application_argument),
      )),
      $.unary_expression,
    ),

    application_argument: $ => choice(
      $.prefixed_argument,
      $.application_atom,
    ),

    prefixed_argument: $ => prec(PREC.unary, seq(
      field("operator", choice("~", "not")),
      field("argument", $.application_atom),
    )),

    unary_expression: $ => choice(
      prec(PREC.unary, seq(
        field("operator", choice("~", "not")),
        field("argument", $.unary_expression),
      )),
      $._atomic_expression,
    ),

    _atomic_expression: $ => choice(
      $.if_expression,
      $.let_expression,
      $.case_expression,
      $.fn_expression,
      $.perform_expression,
      $.handle_expression,
      $.resume_expression,
      $.application_atom,
    ),

    application_atom: $ => choice(
      $.integer_literal,
      $.float_literal,
      $.string_literal,
      $.char_literal,
      $.boolean_literal,
      $.unit_expression,
      $.identifier,
      $.upper_identifier,
      $.qualified_name,
      $.tuple_expression,
      $.list_expression,
      $.parenthesized_expression,
    ),

    if_expression: $ => seq(
      "if",
      field("condition", $._expression),
      "then",
      field("consequence", $._expression),
      "else",
      field("alternative", $._expression),
    ),

    let_expression: $ => seq(
      "let",
      repeat($._declaration),
      "in",
      field("body", $._expression),
      "end",
    ),

    case_expression: $ => prec.right(seq(
      "case",
      field("scrutinee", $._expression),
      "of",
      optional("|"),
      $.case_arm,
      repeat(seq("|", $.case_arm)),
    )),

    case_arm: $ => seq(
      field("pattern", $._pattern),
      "=>",
      field("body", $._expression),
    ),

    fn_expression: $ => seq(
      "fn",
      field("parameter", $._pattern),
      "=>",
      field("body", $._expression),
    ),

    perform_expression: $ => seq(
      "perform",
      field("effect", $.upper_identifier),
      field("argument", $.application_atom),
    ),

    handle_expression: $ => prec.right(seq(
      "handle",
      field("body", $._expression),
      "with",
      "return",
      field("return_variable", $.handler_variable),
      "=>",
      field("return_body", $._expression),
      repeat(seq("|", $.effect_handler)),
    )),

    effect_handler: $ => seq(
      field("effect", $.upper_identifier),
      field("payload", $.handler_variable),
      field("continuation", $.handler_variable),
      "=>",
      field("body", $._expression),
    ),

    resume_expression: $ => seq(
      "resume",
      field("continuation", $.application_atom),
      field("argument", $.application_atom),
    ),

    tuple_expression: $ => seq(
      "(",
      field("first", $._expression),
      ",",
      field("second", $._expression),
      repeat(seq(",", field("rest", $._expression))),
      ")",
    ),

    list_expression: $ => seq(
      "[",
      optional(seq(
        field("element", $._expression),
        repeat(seq(",", field("element", $._expression))),
      )),
      "]",
    ),

    parenthesized_expression: $ => seq(
      "(",
      $._expression,
      ")",
    ),

    unit_expression: _ => seq("(", ")"),

    _pattern: $ => $.annotated_pattern,

    annotated_pattern: $ => choice(
      prec.right(PREC.annotation, seq(
        field("pattern", $.as_pattern),
        ":",
        field("type", $._type_expression),
      )),
      $.as_pattern,
    ),

    as_pattern: $ => choice(
      prec.right(seq(
        field("name", $.identifier),
        "as",
        field("pattern", $.as_pattern),
      )),
      $.cons_pattern,
    ),

    cons_pattern: $ => choice(
      prec.right(PREC.cons, seq(
        field("head", $.application_pattern),
        "::",
        field("tail", $.cons_pattern),
      )),
      $.application_pattern,
    ),

    application_pattern: $ => choice(
      prec.left(seq(
        field("constructor", choice($.qualified_constructor, $.upper_identifier)),
        field("argument", $.atomic_pattern),
      )),
      $.atomic_pattern,
    ),

    atomic_pattern: $ => choice(
      $.wildcard,
      $.identifier,
      $.qualified_constructor,
      $.upper_identifier,
      $.negative_integer_pattern,
      $.negative_float_pattern,
      $.integer_literal,
      $.float_literal,
      $.string_literal,
      $.char_literal,
      $.boolean_literal,
      $.unit_pattern,
      $.tuple_pattern,
      $.list_pattern,
      $.parenthesized_pattern,
    ),

    tuple_pattern: $ => seq(
      "(",
      field("first", $._pattern),
      ",",
      field("second", $._pattern),
      repeat(seq(",", field("rest", $._pattern))),
      ")",
    ),

    list_pattern: $ => seq(
      "[",
      optional(seq(
        field("element", $._pattern),
        repeat(seq(",", field("element", $._pattern))),
      )),
      "]",
    ),

    parenthesized_pattern: $ => seq(
      "(",
      $._pattern,
      ")",
    ),

    unit_pattern: _ => seq("(", ")"),

    negative_integer_pattern: $ => seq("~", $.integer_literal),
    negative_float_pattern: $ => seq("~", $.float_literal),

    _type_expression: $ => $.arrow_type,

    arrow_type: $ => choice(
      prec.right(PREC.type_arrow, seq(
        field("domain", $.tuple_type),
        "->",
        field("codomain", $.arrow_type),
      )),
      $.tuple_type,
    ),

    tuple_type: $ => choice(
      prec.left(PREC.type_tuple, seq(
        field("left", $.tuple_type),
        "*",
        field("right", $.type_application),
      )),
      $.type_application,
    ),

    type_application: $ => choice(
      prec.left(PREC.type_application, seq(
        field("argument", $.type_argument),
        field("constructor", $._type_name),
      )),
      $.type_argument,
    ),

    type_argument: $ => choice(
      $.parenthesized_type,
      $.type_tuple_arguments,
      $.type_variable,
      $._type_name,
    ),

    parenthesized_type: $ => seq(
      "(",
      $._type_expression,
      ")",
    ),

    type_tuple_arguments: $ => seq(
      "(",
      field("first", $._type_expression),
      ",",
      field("second", $._type_expression),
      repeat(seq(",", field("rest", $._type_expression))),
      ")",
    ),

    type_parameters: $ => choice(
      seq(field("parameter", $.type_variable)),
      seq(
        "(",
        field("parameter", $.type_variable),
        repeat(seq(",", field("parameter", $.type_variable))),
        ")",
      ),
    ),

    _type_name: $ => choice(
      $.name,
      $.qualified_name,
    ),

    qualified_name: $ => seq(
      field("head", $.upper_identifier),
      repeat1(seq(".", field("tail", $.name))),
    ),

    qualified_constructor: $ => seq(
      field("head", $.upper_identifier),
      repeat1(seq(".", field("tail", $.upper_identifier))),
    ),

    handler_variable: $ => choice(
      $.identifier,
      $.wildcard,
    ),

    name: $ => choice(
      $.identifier,
      $.upper_identifier,
    ),

    _import_package_name: $ => choice(
      $.upper_identifier,
      $.internal_package_identifier,
    ),

    wildcard: _ => "_",

    boolean_literal: _ => choice("true", "false"),

    identifier: _ => /[a-z][a-zA-Z0-9_]*/,
    internal_package_identifier: _ => /__[A-Za-z][a-zA-Z0-9_]*/,
    upper_identifier: _ => /[A-Z][a-zA-Z0-9_]*/,
    type_variable: _ => token(seq("'", /[a-z][a-zA-Z0-9_]*/)),
    integer_literal: _ => /\d+/,
    float_literal: _ => token(seq(/\d+/, ".", /\d+/)),
    string_literal: _ => token(seq(
      '"',
      repeat(choice(
        /[^"\\]+/,
        /\\./,
      )),
      '"',
    )),
    char_literal: _ => token(seq(
      "'",
      choice(
        /[^'\\]/,
        /\\./,
      ),
      "'",
    )),
  },
});
