# Hiko: System Map

Orientation for the repository: what each crate does, how they depend on each
other, and how a script goes from source to execution. Details live in the
linked documents; this page avoids counts and line numbers because they go
stale.

## Crates

| Crate | Responsibility |
|---|---|
| `hiko-syntax` | Lexer, parser, AST, formatter/pretty-printer, desugaring (including flattening `structure`/`signature`), constant folding |
| `hiko-types` | Hindley-Milner inference with the value restriction, exhaustiveness and redundancy checks |
| `hiko-compile` | Bytecode compiler, opcode set (`op.rs`), chunk format, import loading |
| `hiko-vm` | Bytecode verifier, stack VM, GC, builtins, `VMBuilder`, run config, single-threaded and threaded runtimes |
| `hiko-builtin-meta` | Builtin names, type signatures, and domain metadata shared by the type checker, compiler, and VM |
| `hiko-common` | Shared raw helpers (hashing, HTTP, and similar) used by the VM and CLI |
| `hiko-cli` | `hiko` command: `run`, `check`, `build-vm`, `inspect-work`, `lock` |
| `hiko-docgen` | Static documentation generator for published Hiko libraries |
| `hiko-harness` | Agentic coding tool; runs `tools/*.hml` scripts through `hiko-cli` under a policy |
| `tree-sitter-hiko` | Tree-sitter grammar for editors |

Other top-level directories: `libraries/` (Std and other package sources),
`tools/` (harness tool scripts), `examples/`, `benchmarks/`, `policies/` (run
policies), `specs/` (TLA+ and Quint models).

### Dependency graph

```
hiko-cli ──→ hiko-vm ──→ hiko-compile ──→ hiko-types ──→ hiko-syntax
                │              │               │
                └──────────────┴───────────────┴──→ hiko-builtin-meta

hiko-common  ←── hiko-cli, hiko-vm, hiko-compile, hiko-docgen
hiko-harness ──→ hiko-cli (as a subprocess)
```

## Pipeline

```
source (.hml)
  → lex, parse                       hiko-syntax
  → desugar, flatten modules         hiko-syntax/desugar.rs
  → constant fold                    hiko-syntax/constfold.rs
  → type inference, pattern checks   hiko-types
  → bytecode generation              hiko-compile
  → bytecode verification            hiko-vm/verify.rs
  → execution                        hiko-vm (VM + runtime)
```

Modules exist only at compile time: structures are flattened into mangled
top-level names before inference, so the VM has no module representation. See
[modules.md](../language/modules.md).

## Where to find details

| Topic | Source of truth |
|---|---|
| Language surface, types, operators | [project-state.md](../project/project-state.md), [sml-deltas.md](../language/sml-deltas.md), [hiko.ebnf](../language/hiko.ebnf) |
| Value representation, VM ownership, process creation cost | [vm.md](vm.md), [rust-memory-layout.md](rust-memory-layout.md) |
| Processes, scheduling, async I/O, cancellation | [runtime.md](runtime.md) |
| Limits, `run_slice`, GC behavior | [execution-bounds.md](execution-bounds.md) |
| Builtins and their signatures | [builtins.md](../builtins/builtins.md), `crates/hiko-builtin-meta` |
| Builtin domains, feature gates, `VMBuilder` policy | [builtin-domains.md](../builtins/builtin-domains.md) |
| Host-work and I/O accounting | [accounting-matrix.md](../builtins/accounting-matrix.md) |
| Run config files and `build-vm` | [README](../../README.md), [full-builtin-run-config.example.toml](../builtins/full-builtin-run-config.example.toml) |
| Native host handles | [host-handles.md](host-handles.md) |
| Imports, lockfile, package cache | [modules.md](../language/modules.md) |
| Verifier and formal models | [verification.md](../verification/verification.md) |
| Effects | [effects.md](../language/effects.md) |
