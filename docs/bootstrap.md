# Hiko: Technical Bootstrap Document (v1.0)

**File extension:** `.hml`
**CLI command:** `hiko`

---

## 1. Project Summary

Hiko implements **Core SML**, the core language of Standard ML as defined in the Definition of Standard ML (chapters 2, 4, 6), **excluding the module language** (chapters 3, 5, 7: structures, signatures, functors, sharing constraints).

The implementation target is a bytecode VM written in Rust, designed for scripting use cases where startup time, simplicity, and embeddability matter.

v0 implements the functional nucleus of Core SML first. Some Core SML features (records, `ref`, exceptions, equality types, and full derived forms) are staged into later phases, not because they are unimportant, but because the functional nucleus must be correct before they are layered on. A small number of non-SML conveniences are added for tooling, most notably monomorphic operator names and a simple file-loading mechanism.

v0 prioritizes semantic correctness over Basis compatibility, optimization, and advanced abstraction mechanisms.

Hiko is SML-derived, but not bug-for-bug compatible with SML'97. Known SML defect clusters are triaged explicitly in [sml-deltas.md](./sml-deltas.md), and Hiko prefers documented simplification over inheriting historical ambiguity.

## 2. Core SML Feature Status

### 2.1 Included in v0 (matches Core SML closely)

| Feature                                                                                               | SML core reference |
| ----------------------------------------------------------------------------------------------------- | ------------------ |
| Call-by-value evaluation, strict left-to-right                                                        | §6                 |
| Hindley–Milner type inference (Algorithm W implementation strategy)                                   | §4.5–4.7           |
| Value restriction (only syntactic values generalized)                                                 | §4.7               |
| Algebraic datatypes, single-datatype recursion                                                        | §2.4, §4.2         |
| Pattern matching (left-to-right, first-match, exhaustiveness **error**, redundant clause **warning**) | §2.6, §6.7         |
| Let-polymorphism                                                                                      | §4.6               |
| Lexical scoping and closures                                                                          | §6                 |
| Recursive bindings (`fun`, `val rec`), mutual recursion via `and`                                     | §2.9               |
| Tuples and lists (`::`, `[]`)                                                                         | §2.5               |
| `local ... in ... end`                                                                                | §2.8               |
| Type aliases (`type`)                                                                                 | §2.4               |
| Wildcard, as-patterns, layered patterns                                                               | §2.6               |

### 2.2 Core SML features deferred to later phases

These are part of Core SML but excluded from v0. Each is a deliberate staging decision, not a permanent omission.

| Feature                       | SML core behavior                            | Target      | Why deferred                                                                                                                 |
| ----------------------------- | -------------------------------------------- | ----------- | ---------------------------------------------------------------------------------------------------------------------------- |
| **Records**                   | Structural record types, `#label` projection | Phase 6     | Row typing or restricted records add significant unification complexity; tuples are sufficient to bootstrap                  |
| **`ref` / mutable state**     | `ref`, `:=`, `!`                             | Phase 6     | A pure functional core is simpler to verify; `ref` interacts with the value restriction and requires mutable cells in the VM |
| **Exceptions**                | `exception`, `raise`, `handle`               | Phase 6     | Exceptions require a separate control-flow mechanism in the VM                                                               |
| **Equality types**            | `''a` equality type variables, `eqtype`      | Phase 7     | Requires a parallel kind system for type variables; see §6 for the v0 equality policy                                        |
| **Mutual datatype recursion** | `datatype t1 = ... and t2 = ...`             | Phase 6     | Single-datatype recursion is sufficient for v0; mutual recursion adds complexity to datatype environment setup               |
| **Full derived forms**        | `while`, sequencing, etc.                    | Incremental | Added as needed; `if`/`case`/`let` cover v0                                                                                  |

### 2.2.1 Core SML features permanently excluded

| Feature                  | SML core behavior                   | Why excluded                                                           |
| ------------------------ | ----------------------------------- | ---------------------------------------------------------------------- |
| **Overloaded operators** | `+` works on int, word, real        | Replaced by monomorphic operator names (see §2.3)                      |
| **`abstype`**            | Abstract types in the core language | Effectively dead in SML practice; subsumed by opaque module ascription |
| **Full Basis Library**   | Standard Basis                      | Enormous and module-dependent; Hiko provides its own small stdlib      |

### 2.3 Non-SML divergences in v0

These are deliberate deviations from Core SML.

| Divergence                | SML behavior                                            | Hiko v0 behavior                                                                  | Justification                                         |
| ------------------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------- | ----------------------------------------------------- |
| **Operator names**        | `+`, `*`, etc. are overloaded across int/real/word      | Monomorphic: `+` (int), `+.` (float), `^` (string concat)                         | Avoids ad-hoc overloading machinery                   |
| **Exhaustiveness policy** | Non-exhaustive match is a warning in most SML compilers | Non-exhaustive match is a **compile-time error**                                  | Stronger guarantee; avoids runtime `Match` failures   |
| **Import mechanism**      | No import in Core SML (modules handle composition)      | `use "file.hml"` loads a file and imports its top-level bindings                  | Practical replacement for deferred modules            |
| **Equality**              | Polymorphic `=` with equality type tracking             | `=` restricted to scalar types only in v0                                         | Avoids a partial `eqtype` system                      |
| **Comparison operators**  | `<`, `<=`, etc. are polymorphic across numeric types    | Monomorphic: `<`, `>`, `<=`, `>=` for `int`; `<.`, `>.`, `<=.`, `>=.` for `float` | Keeps operator design consistent with arithmetic      |
| **Type shadowing**        | Allowed; later type bindings shadow earlier ones        | Disallowed in v0; redefining a type name in the same scope is an error            | Simplifies type environment handling                  |
| **Constructor shadowing** | Constructors can be shadowed by value bindings          | Constructors cannot be shadowed by value bindings                                 | Prevents confusing pattern matching interactions      |
| **Surface syntax**        | Standard SML syntax                                     | Minor syntax preferences and implementation-oriented simplifications              | Semantics matter more than full source-level fidelity |

### 2.4 Module language (excluded entirely)

Everything from the Definition's module language (chapters 3, 5, 7):

- `structure`, `signature`, `functor`
- `open`, sharing, `where type`
- Opaque/transparent ascription
- Derived forms that depend on modules

This is the primary scope boundary. The module language is absent, not simplified.

---

## 3. MVP Language Specification

### 3.0 Lexical conventions

- **Comments:** `(* ... *)`, nestable.
- **Identifiers:** alphanumeric starting with a lowercase letter or `_`. Constructor names start with an uppercase letter.
- **Keywords:** `val`, `fun`, `fn`, `let`, `in`, `end`, `if`, `then`, `else`, `case`, `of`, `datatype`, `type`, `local`, `and`, `rec`, `use`, `true`, `false`, `not`, `andalso`, `orelse`, `mod`, `as`.
- **Int literals:** decimal digits. Literals are always unsigned in the lexer; `~42` is parsed as unary negation applied to `42`.
- **Float literals:** must contain `.` or exponent notation (`e`/`E`) to distinguish from Int. Examples: `3.14`, `1.0e10`, `2.5E-3`, `0.0`.
- **String literals:** `"..."` with `\n`, `\t`, `\\`, `\"` escapes.
- **Char literals:** `#"c"`.

### 3.1 Types

**Built-in types:**

- `int`: 64-bit signed integer
- `float`: 64-bit IEEE float (literals support decimal and exponent notation: `3.14`, `1.0e10`, `2.5E-3`)
- `bool`: `true`, `false`
- `string`: UTF-8 immutable string
- `char`: Unicode scalar value
- `unit`: the type `()` with single value `()`

**Type constructors:**

- `a * b * c`: tuple types (2+ elements)
- `'a list`: homogeneous list
- `a -> b`: function type
- User-defined datatypes (possibly parameterized)

**Type variables:** `'a`, `'b`, etc. (SML-style)

### 3.2 Expressions

```sml
e ::= int_lit | float_lit | "s" | #"c"  (* literals *)
    | true | false | ()                 (* constants *)
    | x                                 (* variable *)
    | (e1, e2, ..., en)                 (* tuple, n >= 2 *)
    | [e1, e2, ..., en]                 (* list literal, sugar for cons/nil *)
    | e1 :: e2                          (* cons *)
    | e1 op e2                          (* binary operator *)
    | ~e                                (* unary negation *)
    | not e                             (* boolean not *)
    | e1 e2                             (* application *)
    | fn x => e                         (* lambda *)
    | if e1 then e2 else e3             (* conditional *)
    | let d1 ... dn in e end            (* let block *)
    | case e of p1 => e1 | ... | pn => en
    | (e)                               (* parenthesized *)
    | e : t                             (* type annotation *)
```

**Note:** v0 does **not** include tuple projection syntax like `#1 e`. In SML, projection is part of the record system. Since records are deferred in Hiko v0 (§2.2), projection syntax is also deferred. Tuple decomposition is done through pattern matching and `val` bindings.

### 3.3 Declarations

```sml
d ::= val p = e
    | val rec f = fn x => e
    | fun f x1 x2 ... xn = e
    | fun f p11 ... p1n = e1                 (* clausal, multi-arg *)
        | f p21 ... p2n = e2
        | ...
    | datatype <tyvars> T = C1 of t | C2 | ...
    | type <tyvars> T = t
    | local d1 in d2 end
```

Representative declaration forms:

```sml
datatype T = C1 of t1 | C2
datatype 'a T = C1 of 'a | C2
datatype ('a, 'b) T = C1 of 'a | C2 of 'b
type T = t
type 'a T = t
type ('a, 'b) T = t
```

**Mutual recursion via `and`:**

```sml
fun f x = ... g ...
and g y = ... f ...
```

**Datatype syntax (type parameters are declared before the type name):**

```sml
datatype 'a option = None | Some of 'a
datatype ('a, 'b) either = Left of 'a | Right of 'b
datatype shape = Circle of float | Rect of float * float
```

**Type alias syntax uses the same parameter binder style as `datatype`:**

```sml
type 'a box = 'a
type ('a, 'b) pair = 'a * 'b
```

### 3.4 Patterns

```sml
p ::= _
    | x
    | int_lit | float_lit | "s" | true | false | ()
    | (p1, p2, ..., pn)
    | C
    | C p
    | p1 :: p2
    | [p1, ..., pn]
    | p : t
    | x as p
```

### 3.5 Programs and Imports

A `.hml` file is a sequence of top-level declarations, evaluated in order. No header or module declaration is required. Each declaration group is elaborated and evaluated before the next top-level declaration group; later declarations are not visible to earlier ones.

**Import:**

```sml
use "path/to/file.hml"
```

Import semantics:

1. The file path is resolved relative to the importing file's directory.
2. A file is loaded at most once per compilation.
3. Circular imports are compile-time errors.
4. After loading, the imported file's top-level bindings are added to the importing environment.
5. If multiple imported files define the same top-level name in the same namespace, that is a **compile-time error**.
6. A local declaration in the importing file may shadow an imported value binding, following normal lexical shadowing rules.

This is not textual inclusion.

### 3.6 Namespaces and shadowing

Hiko follows the Standard ML split between **type-level names** and **value-level names**.

- **Type names** live in a separate namespace from **value names**.
- **Constructors** live in the **value namespace**. Constructor names cannot be shadowed by value bindings. This is stricter than SML but prevents confusing interactions between pattern matching and local bindings. Implementation note: the environment must track constructor bindings distinctly from ordinary value bindings so this rule can be enforced during name resolution and reported clearly in diagnostics.
- A value binding may shadow an earlier value binding in a nested lexical scope.
- A type alias or datatype declaration may not redefine an existing type name in the same scope.
- At top level within a single file, duplicate definitions in the same namespace are compile-time errors.
- Imported duplicate names in the same namespace are compile-time errors.

### 3.7 Operator precedence (highest to lowest)

| Precedence | Operators                                                 | Associativity |
| ---------- | --------------------------------------------------------- | ------------- |
| 7          | function application                                      | left          |
| 6          | `~`, `not`                                                | prefix        |
| 5          | `*`, `/`, `*.`, `/.`, `mod`                               | left          |
| 4          | `+`, `-`, `+.`, `-.`, `^`                                 | left          |
| 3          | `::`                                                      | right         |
| 2          | `=`, `<>`, `<`, `>`, `<=`, `>=`, `<.`, `>.`, `<=.`, `>=.` | non-assoc     |
| 1          | `andalso`                                                 | right         |
| 0          | `orelse`                                                  | right         |

---

## 4. Example Programs

### Algebraic Data Types

```sml
datatype shape =
    Circle of Float
  | Rect of Float * Float

fun area s =
  case s of
    Circle r    => 3.14159 *. r *. r
  | Rect (w, h) => w *. h

val _ = println (float_to_string (area (Circle 5.0)))
```

### Parameterized Datatypes

```sml
datatype 'a option = None | Some of 'a

fun map_option f opt =
  case opt of
    None   => None
  | Some x => Some (f x)
```

### Lists and Higher-Order Functions

```sml
fun map f xs =
  case xs of
    []      => []
  | x :: xs => f x :: map f xs

fun filter p xs =
  case xs of
    []      => []
  | x :: xs =>
      if p x then x :: filter p xs
      else filter p xs

fun foldl f acc xs =
  case xs of
    []      => acc
  | x :: xs => foldl f (f (acc, x)) xs

val sum = foldl (fn (acc, x) => acc + x) 0 [1, 2, 3, 4, 5]
```

### Closures and Composition

```sml
fun compose f g = fn x => f (g x)

val inc = fn x => x + 1
val double = fn x => x * 2
val inc_then_double = compose double inc

val _ = println (int_to_string (inc_then_double 3))  (* 8 *)
```

### File Import

```sml
(* file: math.hml *)
fun square x = x * x

(* file: main.hml *)
use "math.hml"
val _ = println (int_to_string (square 5))  (* 25 *)
```

### Float Comparisons

```sml
fun clamp (lo : Float) (hi : Float) (x : Float) =
  if x <. lo then lo
  else if x >. hi then hi
  else x
```

---

## 5. Architecture

```text
Source (.hml)
    │
    ▼
Lexer
    │
    ▼
Parser
    │
    ▼
Desugaring
    │
    ▼
Type inference
    │
    ▼
Match compilation
    │
    ▼
Codegen
    │
    ▼
VM
```

### No separate IR

For v0, the typed Core AST is the IR. A separate SSA/ANF IR would help with optimization but does not earn its keep until tail-call optimization or inlining is implemented. The typed AST is walked directly by codegen.

### Error reporting

Every AST node carries a `Span { file_id, start, end }`. Errors are structured diagnostics rendered with source context using `codespan-reporting`.

---

## 6. Equality in v0

**Policy: `=` and `<>` are defined only on scalar types.**

Supported:

- `int`
- `float`
- `bool`
- `char`
- `string`

Not supported:

- Tuples
- Lists
- ADT values
- Functions

Attempting to use `=` on an unsupported type is a **type error**.

SML tracks equality via `''a` and `eqtype`. Hiko v0 does not implement that machinery. Rather than allowing unsound structural equality, v0 restricts equality to scalars.

Float equality and ordering follow IEEE-754 semantics. In particular, `NaN <> NaN` is `true` and `NaN = NaN` is `false`.

```sml
fun list_eq eq xs ys =
  case (xs, ys) of
    ([], [])           => true
  | (x :: xs, y :: ys) => eq x y andalso list_eq eq xs ys
  | _                  => false
```

---

## 7. Value Restriction and Syntactic Values

A type is generalized at a `let`-binding only if the right-hand side is a **syntactic value**.

For v0, syntactic values are:

```sml
v ::= literal
    | x
    | fn x => e
    | C
    | C v
    | (v1, ..., vn)
    | [v1, ..., vn]
```

Examples:

- `fn x => x` → value, may generalize
- `Some (fn x => x)` → value, may generalize
- `(1, fn x => x)` → value, may generalize
- `f x` → not a value
- `if c then e1 else e2` → not a value
- `case e of ...` → not a value

This is the rule used by the type checker, not an informal guideline.

---

## 8. Phased Implementation Roadmap

### Phase 0: Repository Setup and Crate Layout

Success criteria:

- `cargo build`
- `cargo test`
- `cargo clippy`
- CI on push

### Phase 1: Lexer + Parser + AST

Success criteria:

- Parse all surface syntax from §3
- Round-trip parse/pretty/parse
- Golden tests
- Useful syntax diagnostics

Representative test: `fun map f xs = case xs of [] => [] | x :: xs => f x :: map f xs` parses and pretty-prints correctly.

Main risk: operator precedence for `::` vs arithmetic, solved by a precedence-climbing parser.

### Phase 2: Type Inference

Success criteria:

- HM inference for all v0 forms
- Let-polymorphism and value restriction
- Datatype constructor typing
- Equality restriction enforced
- Good unification errors

Representative test: `let val id = fn x => x in (id 42, id true) end` infers `Int * Bool`.

Main risk: getting generalization/instantiation right at `let` boundaries.

### Phase 3: Bytecode Compiler + VM

Success criteria:

- Functions, closures, recursion, arithmetic, let-bindings
- Correct immutable flat closure capture
- Safe stack overflow handling
- Tail-call optimization is optional in the first working VM; if implemented initially, self-tail-calls are sufficient

Representative test: `fun fib n = if n < 2 then n else fib (n - 1) + fib (n - 2)`. `fib 30` returns `832040`.

Main risk: closure capture correctness when closures escape their defining scope.

### Phase 4: ADTs + Pattern Matching

Success criteria:

- Runtime ADT construction and matching
- Exhaustiveness as compile-time error
- Redundancy as warning
- Decision-tree compilation

Representative test: `datatype expr = Num of Int | Add of expr * expr` with a recursive `eval` function.

Main risk: Maranget's algorithm is well-documented but tricky to implement correctly with nested patterns.

### Phase 5: Imports, REPL, Stdlib Basics

Success criteria:

- `use` works with cycle detection and duplicate-name checking
- REPL with persistent environments
- Minimal stdlib

Runtime builtins (implemented in Rust, always available): `print`, `println`, `int_to_string`, `float_to_string`, `string_length`, `panic`. Stdlib functions (written in Hiko, loaded from `libraries/Std-v0.1.0/modules/`): `map`, `filter`, `foldl`, and other list utilities.

Representative test: `use "math.hml"` followed by calling a function defined in that file.

Main risk: REPL state management across re-definitions.

---

## 9. Rust Workspace Layout

```text
hiko/
├── crates/
│   ├── hiko-syntax/
│   ├── hiko-types/
│   ├── hiko-compile/
│   ├── hiko-vm/
│   └── hiko-cli/
├── libraries/
├── tests/
└── examples/
```

Dependency graph:

```text
hiko-cli → hiko-compile → hiko-types → hiko-syntax
              ↓
           hiko-vm
```

---

## 10. VM Design

### 10.1 Instruction Set

v0 uses **immutable flat closures**: captured values are copied into the closure at creation time. Upvalues are read-only.

```rust
#[repr(u8)]
pub enum Op {
    Const,
    Unit,
    True,
    False,

    GetLocal,
    SetLocal,

    GetUpvalue,

    GetGlobal,
    SetGlobal,

    Pop,
    Dup,

    AddInt, SubInt, MulInt, DivInt, ModInt, NegInt,
    AddFloat, SubFloat, MulFloat, DivFloat, NegFloat,

    EqInt, NeInt, LtInt, GtInt, LeInt, GeInt,
    EqFloat, NeFloat, LtFloat, GtFloat, LeFloat, GeFloat,
    EqBool, NeBool,
    EqChar, NeChar,
    EqString, NeString,

    ConcatString,
    Not,

    MakeTuple,
    GetField,

    MakeData,
    GetTag,

    Jump,
    JumpIfFalse,
    JumpIfTag,

    MakeClosure,
    Call,
    TailCall,
    Return,

    CallBuiltin,

    Halt,
    Panic,
}
```

### 10.2 Locals and globals

`SetLocal` is used to initialize local slots during binding evaluation. `SetGlobal` is used only during top-level initialization when a file is being loaded. Neither is a user-visible reassignment operation. All bindings are **immutable**.

### 10.3 Chunk, constants, and function prototypes

```rust
pub struct Chunk {
    pub code: Vec<u8>,
    pub constants: Vec<Constant>,
    pub spans: Vec<(usize, Span)>,
}

pub enum Constant {
    Int(i64),
    Float(f64),
    String(String),
    Char(char),
    Function(FunctionProto),
}

pub struct FunctionProto {
    pub name: Option<String>,
    pub arity: u8,
    pub n_captures: u8,
    pub chunk: Chunk,
}
```

`FuncRef(u16)` (§11.2) is an index into the constant pool's `Function` entries.

### 10.4 Field access convention

`GetField` is used for both tuples and ADT payloads:

- tuple field `i` means the `i`th tuple element
- ADT field `i` means the `i`th payload slot of the constructor

Both are stored in order and accessed by zero-based runtime index.

---

## 11. Runtime Representation

### 11.1 Value

```rust
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
    String(GcRef<HikoString>),
    Tuple(GcRef<HikoTuple>),
    Data(GcRef<HikoData>),
    Closure(GcRef<Closure>),
    Builtin(BuiltinFn),
}
```

### 11.2 Heap Objects

```rust
pub enum HeapObject {
    String(HikoString),
    Tuple(HikoTuple),
    Data(HikoData),
    Closure(Closure),
}

pub struct HikoString(pub String);
pub struct HikoTuple(pub Vec<Value>);

pub struct HikoData {
    pub tag: u16,
    pub fields: Vec<Value>,
}

pub struct FuncRef(pub u16);

pub struct Closure {
    pub proto: FuncRef,
    pub captures: Vec<Value>,
}
```

### 11.3 Lists as built-in ADT

The compiler pre-registers a built-in `list` datatype:

- `Nil`: tag 0, arity 0
- `Cons`: tag 1, arity 2

List literals desugar to nested `Cons(..., Nil)` and compile through normal ADT machinery. `Nil` and `Cons` are internal runtime tags; surface syntax uses `[]` and `::` exclusively.

### 11.4 VM structure

```rust
pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    globals: HashMap<String, Value>,
    heap: Heap,
}

pub struct CallFrame {
    closure: GcRef<Closure>,
    ip: usize,
    base: usize,
}
```

The VM uses a shared value stack. Each `CallFrame` records a `base` index; the frame's locals are `stack[base..base+n_locals]`. Arguments are passed on the stack; the callee's `base` is set so that argument slots become its first locals.

### 11.5 GC strategy

Mark-and-sweep with index-based `GcRef<T>` into `Vec<Option<HeapObject>>`. No raw pointers, no trait objects, no `unsafe` required for the initial implementation.

---

## 12. Type System Plan

### 12.1 Type Representation

```rust
pub enum Type {
    Con(TyCon),
    Var(TyVar),
    App(TyCon, Vec<Type>),
    Arrow(Box<Type>, Box<Type>),
    Tuple(Vec<Type>),
}

pub struct TyVar(pub u32);

pub struct TyCon {
    pub name: String,
    pub arity: usize,
}

pub struct Scheme {
    pub vars: Vec<TyVar>,
    pub ty: Type,
}
```

### 12.2 Algorithm W

Implementation choices:

- substitution as `HashMap<TyVar, Type>`
- occurs-checking unification
- generalization at `let`
- value restriction as defined in §7 (Value Restriction)
- expression annotations unify inferred and declared type

### 12.3 Datatype Handling

For:

```sml
datatype ('a, 'b) T = C1 of t1 | C2 of t2 | C3
```

the compiler:

1. registers `T` with arity 2
2. computes constructor schemes
3. inserts constructors into the value environment
4. rejects unbound type variables inside constructor payload types

### 12.4 Exhaustiveness Checking

Use Maranget's matrix-based usefulness algorithm.

Supported in v0:

- constructors (including nested)
- wildcards / variables
- literals
- tuples
- built-in list patterns

Literal exhaustiveness rules:

- `bool` and `unit` are finite literal domains. The checker recognizes exhaustive coverage (e.g., `true | false` is complete).
- `int`, `float`, `char`, and `string` are open-ended. A wildcard or variable fallback is always required for exhaustive coverage.

**Non-exhaustive match:** compile-time error
**Redundant clause:** warning

---

## 13. Clausal `fun` Desugaring

Multi-clause function definitions are desugared through a **single `case` on the tuple of arguments**.

Example:

```sml
fun f [] y = y
  | f (x :: xs) y = f xs (x :: y)
```

desugars to:

```sml
val rec f =
  fn x =>
    fn y =>
      case (x, y) of
        ([], y) => y
      | (x :: xs, y) => f xs (x :: y)
```

This makes exhaustiveness and redundancy checking precise and uniform.

---

## 14. REPL Semantics

The REPL maintains persistent type and value environments across inputs.

- Later REPL bindings may shadow earlier value bindings.
- Redefining a type name in the same REPL session is a compile-time error.
- Imported duplicate names remain errors.

---

## 15. Testing Plan

Test categories:

- parser golden tests
- parser round-trip tests
- type inference tests
- runtime execution tests
- error snapshot tests
- exhaustiveness/redundancy tests

Key areas:

- literals and arithmetic
- let-polymorphism and value restriction
- closures
- tuples and lists
- parameterized datatypes
- pattern matching
- equality restriction
- imports and collision behavior

---

## 16. Final Recommendation

Build in this order:

1. syntax
2. type inference
3. VM
4. ADTs + match compilation
5. imports + REPL + stdlib

Key design bets:

- hand-written parser
- typed AST as first IR
- immutable flat closures
- index-based GC
- lists as built-in ADT
- scalar-only equality
- non-exhaustive match as error

**Start here:** create the workspace and implement the lexer and parser first. The first milestone is parsing a complete `.hml` file into an AST and pretty-printing it back without losing structure.
