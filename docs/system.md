# Hiko: System Description

## What is Hiko

Hiko is a strict, statically typed, ML-family scripting language implemented in Rust. It targets sandboxed agent scripting — running untrusted code with policy-controlled capabilities, type-checked at compile time, executing on a bytecode VM with a mark-and-sweep garbage collector.

The language semantics are anchored in Core SML (Standard ML), with Hindley-Milner type inference, algebraic data types, exhaustive pattern matching, and OCaml 5-inspired algebraic effect handlers for structured control flow.

## Current state

- **26,136 lines of Rust** across 7 crates
- **230 tests** (lexer, parser, type inference, exhaustiveness, VM, stdlib)
- **67 builtins** (string, math, file, HTTP, JSON, regex, exec, RNG, bytes)
- **57 bytecode opcodes** including tail calls and algebraic effects
- **48 examples**, 6 benchmarks, 7 tool scripts, 4 stdlib modules
- **Published on crates.io** as v0.3.0

## Crate architecture

```
hiko/
├── crates/
│   ├── hiko-syntax/     Lexer, parser, AST, pretty-printer, desugaring
│   ├── hiko-types/      HM type inference, exhaustiveness checking
│   ├── hiko-compile/    Bytecode compiler, opcodes, chunk format
│   ├── hiko-vm/         Stack VM, GC, builtins, config, builder
│   ├── hiko-cli/        CLI (run, check, build-vm)
│   └── hiko-harness/    Agentic coding tool (LLM loop, tools, config)
├── stdlib/              list.hml, option.hml, either.hml, json.hml
├── tools/               Harness tool scripts (.hml)
├── examples/            48 example programs
├── benchmarks/          Manticore-derived benchmarks
├── scripts/             release.hml
├── configs/             Run config TOML files
└── docs/                bootstrap.md, runtime.md, system.md
```

### Dependency graph

```
hiko-cli ──→ hiko-compile ──→ hiko-types ──→ hiko-syntax
                  │
                  ▼
              hiko-vm

hiko-harness ──→ hiko-vm, hiko-compile, hiko-syntax
```

## Compilation pipeline

```
Source (.hml)
    │
    ▼
Lexer (hiko-syntax/lexer.rs, 780 lines)
    │  Tokenizes source into Token stream
    │  Handles: strings, chars, numbers, identifiers, keywords, operators
    ▼
Parser (hiko-syntax/parser.rs, 1629 lines)
    │  Recursive descent with precedence climbing
    │  8 precedence levels: orelse > andalso > comparison > cons > add > mul > app > atom
    ▼
Desugar (hiko-syntax/desugar.rs, 324 lines)
    │  List literals → cons chains
    │  andalso/orelse → if-then-else
    │  not → if-then-else
    ▼
Const fold (hiko-syntax/constfold.rs, 172 lines)
    │  Compile-time constant evaluation
    ▼
Type inference (hiko-types/infer.rs, 1934 lines)
    │  Algorithm W (Hindley-Milner)
    │  Value restriction (SML-97 style)
    │  Exhaustiveness checking (Maranget's algorithm)
    ▼
Compiler (hiko-compile/compiler.rs, 1226 lines)
    │  Two-pass: type inference → codegen
    │  57 bytecode opcodes
    │  Tail-call optimization through if/case/let
    ▼
VM (hiko-vm/vm.rs, 1491 lines)
    Bytecode interpreter
    Mark-and-sweep GC
    Algebraic effect handlers
```

## Type system

Hindley-Milner with the SML-97 value restriction.

### Built-in types

| Type | Representation | Size |
|---|---|---|
| `int` | 64-bit signed | 8 bytes |
| `float` | 64-bit IEEE 754 | 8 bytes |
| `bool` | boolean | 1 byte |
| `char` | Unicode scalar | 4 bytes |
| `string` | UTF-8 immutable | heap-allocated |
| `bytes` | raw byte sequence | heap-allocated |
| `rng` | opaque PRNG state | heap-allocated |
| `unit` | singleton `()` | 0 bytes |

### Type constructors

- `'a list` — cons-list with fixed tags (Nil=0, Cons=1)
- `a * b` — tuples
- `a -> b` — functions
- User-defined algebraic data types

### Equality types

`=` and `<>` work on: `int`, `float`, `bool`, `string`, `char`, `unit`, `bytes`, type variables, and tuples of equality types.

### Polymorphism

Generalization at `val` bindings of syntactic values only. Type annotations supported. No overloading — monomorphic operators (`+` for `int`, `+.` for `float`, `^` for `string`).

## Runtime representation

### Value (16 bytes, Copy, no Drop)

```rust
enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
    Heap(GcRef),      // 4-byte index into GC arena
    Builtin(u16),     // index into builtin table
}
```

### Heap objects

```rust
enum HeapObject {
    String(String),
    Bytes(Vec<u8>),
    Tuple(SmallVec<[Value; 2]>),
    Data { tag: u16, fields: SmallVec<[Value; 2]> },
    Rng { state: u64, inc: u64 },
    Closure { proto_idx: usize, captures: Rc<[Value]> },
    Continuation { saved_frames: Vec<SavedFrame>, saved_stack: Vec<Value> },
}
```

`SmallVec<[Value; 2]>` inlines up to 2 values (32 bytes) without heap allocation, covering cons cells, pairs, and most constructors.

### Garbage collector

Mark-and-sweep with worklist-based marking (avoids stack overflow on deep object graphs), free-list reuse, and adaptive collection threshold. Per-process — each VM instance has its own heap and collects independently.

## VM architecture

### Stack-based execution

- `Vec<Value>` operand stack (up to 64K values)
- `Vec<CallFrame>` frame stack (up to 65,536 frames)
- `Vec<HandlerFrame>` for algebraic effect dispatch
- Direct opcode dispatch with `match` statement

### Opcodes (57)

| Category | Opcodes |
|---|---|
| Constants | `Const`, `Unit`, `True`, `False` |
| Variables | `GetLocal`, `SetLocal`, `GetUpvalue`, `GetGlobal`, `SetGlobal` |
| Stack | `Pop` |
| Int arithmetic | `AddInt`, `SubInt`, `MulInt`, `DivInt`, `ModInt`, `Neg` |
| Float arithmetic | `AddFloat`, `SubFloat`, `MulFloat`, `DivFloat`, `NegFloat` |
| Int comparison | `LtInt`, `GtInt`, `LeInt`, `GeInt` |
| Float comparison | `LtFloat`, `GtFloat`, `LeFloat`, `GeFloat` |
| String | `ConcatString` |
| Equality | `Eq`, `Ne`, `Not` |
| Data | `MakeTuple`, `GetField`, `MakeData`, `GetTag` |
| Control | `Jump`, `JumpIfFalse`, `Call`, `TailCall`, `CallDirect`, `TailCallDirect`, `Return`, `Halt` |
| Closures | `MakeClosure` |
| Effects | `InstallHandler`, `Perform`, `Resume`, `RemoveHandler` |
| Error | `Panic` |

### Tail-call optimization

The `TailCall` opcode reuses the current call frame. Propagated through `if`/`case`/`let` branches so tail-recursive functions run in constant stack space.

### Algebraic effect handlers

Shallow, one-shot delimited continuations. `Perform` captures the stack and frames between the perform site and the nearest matching handler. `Resume` restores the captured continuation. Supports nested handlers, recursive handlers, and the generator/state/error patterns.

## Builtins (74)

### String (15)

`print`, `println`, `read_line`, `read_stdin`, `string_length`, `substring`, `string_contains`, `split`, `trim`, `string_replace`, `starts_with`, `ends_with`, `to_upper`, `to_lower`, `string_join`, `regex_match`, `regex_replace`

### Conversion (6)

`int_to_string`, `float_to_string`, `string_to_int`, `char_to_int`, `int_to_char`, `int_to_float`

### Math (5)

`sqrt`, `abs_int`, `abs_float`, `floor`, `ceil`

### JSON (7)

`json_parse` (String → json ADT via serde_json), `json_to_string` (json ADT → String), `json_get`, `json_keys`, `json_length` (string-based convenience)

### Filesystem (13)

`read_file`, `read_file_bytes`, `write_file`, `file_exists`, `list_dir`, `remove_file`, `create_dir`, `is_dir`, `is_file`, `glob`, `walk_dir`

### Path (1)

`path_join`

### Hashline (2)

`read_file_tagged` (FNV-1a content hashes per line), `edit_file_tagged` (hash-verified edits)

### HTTP (5)

`http_get` (simple GET), `http` (any method + headers + body → text), `http_json` (→ json), `http_msgpack` (→ json via rmp-serde), `http_bytes` (→ Bytes)

### Bytes (5)

`bytes_length`, `bytes_get`, `bytes_slice`, `bytes_to_string`, `string_to_bytes`

### RNG (4)

`random_bytes` (OS entropy via dryoc), `rng_seed` (Bytes → Rng, PCG-XSH-RR), `rng_int` ((Rng, Int) → (Int, Rng)), `rng_bytes` ((Rng, Int) → (Bytes, Rng))

### System (5)

`getenv`, `epoch`, `exec` (whitelisted commands with timeout), `sleep`, `exit`

### Testing (3)

`panic`, `assert`, `assert_eq`

## Run config system

Run configs define VM capabilities at compile time for generated binaries. A config TOML file is read by `hiko build-vm` and baked into a standalone Rust binary as hardcoded VMBuilder calls.

```toml
[limits]
max_fuel = 10_000_000
max_heap = 500_000

[capabilities.stdio.println]
enabled = true

[capabilities.filesystem.read_file]
enabled = true
folders = ["."]

[capabilities.exec.exec]
enabled = true
allowed_commands = ["/usr/bin/mise"]
timeout = 30

[capabilities.http.http]
enabled = true
allowed_hosts = ["api.example.com"]
```

### build-vm codegen

```
config.toml → hiko build-vm → hiko-vm-{name}/
                                ├── Cargo.toml     (crates.io deps)
                                └── src/main.rs    (config baked in)
```

The generated `src/main.rs` is the auditable source of truth. Builtins that the config doesn't allow are not compiled into the binary — there are no runtime checks to bypass.

## VMBuilder

The builder API controls which builtins are registered:

```rust
VMBuilder::new(compiled)
    .with_core()                    // string, math, conversion, etc.
    .with_filesystem(policy)        // read/write/delete per policy
    .with_http(policy)              // all HTTP builtins
    .with_exec(policy)              // whitelisted commands + timeout
    .with_exit()                    // exit builtin
    .max_fuel(10_000_000)           // opcode execution limit
    .max_heap(500_000)              // heap object limit
    .build()
```

Heap size and fuel are configurable. The VM also enforces fixed hard guards of
`hiko_vm::DEFAULT_MAX_STACK_SLOTS` (`65536` value-stack slots) and
`hiko_vm::DEFAULT_MAX_CALL_FRAMES` (`65536` call frames). Those two limits are
currently runtime constants rather than config or `VMBuilder` settings.

## Hashline edit system

Content-hash anchored file editing for LLM agents.

`read_file_tagged` returns lines with 2-char FNV-1a base62 content hashes:

```
1:Kv	(* Factorial using clausal function definition *)
2:ZD	
3:QY	fun fact 0 = 1
4:3v	  | fact n = n * fact (n - 1)
```

`edit_file_tagged` accepts edits referencing line:hash anchors. If the file changed since the last read, hashes won't match and the edit is rejected:

```
R 3:QY fun fact 0 = 1         (* replace line 3 *)
I 4:3v   | fact n = n + 1     (* insert after line 4 *)
D 2:ZD                        (* delete line 2 *)
```

## hiko-harness

Agentic coding tool powered by hiko scripts. The LLM calls tools, each tool is a `.hml` script executed in the VM.

### Architecture

```
LLM (OpenAI-compatible API)
    │ SSE streaming
    ▼
Agent loop (agent.rs)
    │ tool calls
    ▼
Tool registry (tools.rs)
    │ loads .hml scripts
    ▼
hiko-cli runner (per invocation)
    │ run config + sandboxed execution
    ▼
Tool result → back to LLM
```

### Configuration

`hiko-harness.toml` with multi-provider support:

```toml
[default]
model = "gpt-4o"
provider = "openai"

[hiko]
bin = "hiko-cli"
config = "policies/harness-tools.policy.toml"
strict = true

[providers.openai]
api_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[providers.ollama]
api_url = "http://localhost:11434/v1"
api_key_env = "OLLAMA_API_KEY"

[models]
gpt-4o = { provider = "openai", id = "gpt-4o" }
qwen3-32b = { provider = "ollama", id = "qwen3:32b" }

[roles]
default = "gpt-4o"
fast = "gpt-4o-mini"
local = "qwen3-32b"
```

### Tool scripts

```
tools/
├── read.hml     Hashline-tagged file reading
├── edit.hml     Hash-verified file editing
├── find.hml     Glob file search
├── grep.hml     Regex search across files
├── write.hml    Write file content
├── list.hml     Directory listing
└── bash.hml     Whitelisted command execution
```

Tool metadata parsed from comment headers:

```sml
(* tool: read
 * description: Read a file with hashline content-hash anchors
 * param path: string - File path to read
 *)
```

## Stdlib

| Module | Functions |
|---|---|
| `list.hml` | `map`, `filter`, `foldl`, `foldr`, `length`, `reverse`, `append`, `nth`, `zip`, `take`, `drop`, `all`, `any`, `find` |
| `option.hml` | `is_some`, `is_none`, `map_option`, `get_or`, `flat_map_option` |
| `either.hml` | `map_right`, `map_left`, `is_left`, `is_right`, `from_left`, `from_right` |
| `json.hml` | `datatype json = JNull \| JBool of bool \| JInt of int \| JFloat of float \| JStr of string \| JArray \| JObject` |

## JSON support

Two-layer API:

**Typed (via stdlib/json.hml):**

```sml
use "stdlib/json.hml"
val data = json_parse body
val _ = case data of
    JObject fields => ...
  | JArray items => ...
  | JStr s => ...
```

`json_parse` uses `serde_json` in Rust, returns the result as hiko algebraic data type values with fixed constructor tags. 5.4x faster than Python for JSON processing.

**String-based (convenience):**

```sml
val name = json_get (body, "repo.url")
val keys = json_keys body
val n = json_length body
```

## Performance

### Benchmarks (Manticore-derived)

| Benchmark | Input | Result |
|---|---|---|
| ack | (3, 9) | 4093 |
| tak | (24, 16, 8) | 9 |
| nqueens | 8 | 92 solutions |
| primes | <200 | 46 primes |
| ec_ack (effects) | (3, 7) | 1021 |
| ec_fib (effects) | 30 | 832040 |

### JSON processing

```
hiko (release):  4.1ms average
Python:          21.9ms average
Speedup:         5.4x
```

Measured with `hyperfine`, parsing 50 JSON objects from a local file, extracting fields, printing results.

### Startup time

Full pipeline (lex → parse → typecheck → compile → VM boot → execute) in release mode: **under 10ms** for typical scripts.

## Rust dependencies

| Crate | Used for |
|---|---|
| `serde` + `toml` | Run config TOML parsing |
| `serde_json` | JSON builtins |
| `rmp-serde` | MessagePack decoding |
| `ureq` | HTTP client |
| `regex` | Regex builtins |
| `glob` | File pattern matching |
| `smallvec` | Inline tuple/data storage |
| `dryoc` | Cryptographic RNG (OS entropy) |
| `codespan-reporting` | Diagnostic error messages |

## Future work

### Module system (issue #9)

SML-style structures and signatures without functors. Structures are compile-time namespaces — no runtime representation, no VM changes.

```sml
structure Http = struct
  val get = ...
end
Http.get url
```

### Erlang-style process runtime (issue #10)

Multi-process execution with isolated VMs, message passing, per-process GC. Effects stay local to each process. See `docs/runtime.md` for the full design.

Key design decisions:
- One heap per process, per-process GC
- `SendableValue` with `Arc<str>` shared leaves for zero-copy string passing
- Pluggable scheduler trait
- Pluggable I/O backend trait
- No shared mutable state, no concurrent GC
