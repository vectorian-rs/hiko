"use" @keyword
"import" @keyword
"val" @keyword
"rec" @keyword
"fun" @keyword
"and" @keyword
"datatype" @keyword
"type" @keyword
"local" @keyword
"in" @keyword
"end" @keyword
"signature" @keyword
"sig" @keyword
"structure" @keyword
"struct" @keyword
"effect" @keyword
"of" @keyword
"if" @keyword
"then" @keyword
"else" @keyword
"let" @keyword
"case" @keyword
"fn" @keyword
"handle" @keyword
"with" @keyword
"return" @keyword
"perform" @keyword
"resume" @keyword
"as" @keyword

(comment) @comment

(string_literal) @string
(char_literal) @string.special

(integer_literal) @number
(float_literal) @number.float
(boolean_literal) @constant.builtin.boolean

(type_variable) @type.definition

(application_atom
  (identifier) @variable)

(atomic_pattern
  (identifier) @variable)

(handler_variable
  (identifier) @variable)

(type_argument
  (name
    (identifier) @type))

(type_argument
  (name
    (upper_identifier) @type))

(type_application
  constructor: (name
    (identifier) @type))

(type_application
  constructor: (name
    (upper_identifier) @type))

(constructor_declaration
  name: (upper_identifier) @constructor)

(application_atom
  (upper_identifier) @constructor)

(application_pattern
  constructor: (upper_identifier) @constructor)

(atomic_pattern
  (upper_identifier) @constructor)

(effect_declaration
  name: (upper_identifier) @function.special)

(perform_expression
  effect: (upper_identifier) @function.special)

(effect_handler
  effect: (upper_identifier) @function.special)

(structure_declaration
  name: (upper_identifier) @module)

(signature_declaration
  name: (upper_identifier) @module)

(import_declaration
  package: (upper_identifier) @namespace
  module: (upper_identifier) @module)

(qualified_name
  head: (upper_identifier) @namespace)

(qualified_name
  tail: (name
    (upper_identifier) @module))

(qualified_name
  tail: (name
    (identifier) @variable.member))

(qualified_constructor
  head: (upper_identifier) @namespace
  tail: (upper_identifier) @constructor)

(function_binding
  name: (identifier) @function)

(value_rec_declaration
  name: (identifier) @function)

(signature_specification
  name: (identifier) @function)

(type_alias_declaration
  name: (name
    (identifier) @type))

(datatype_declaration
  name: (name
    (identifier) @type))

(datatype_declaration
  name: (name
    (upper_identifier) @type))
