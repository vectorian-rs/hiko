# Docs Index for Agents

Read this file first. Its job is routing, not explanation.

If you need repo-wide product context before loading docs, skim
[README.md](../README.md). There is no separate PRD in this repo; the closest
design-intent document is [whitepaper.md](whitepaper.md).

Audit snapshot on 2026-04-21:

- Docs are compact and mostly design/architecture focused.
- The issue tracker is active, not near zero: 27 open issues, mostly roadmap and
  implementation follow-up rather than triaged operational incidents.
- Recent work is concentrated on runtime/VM correctness, cooperative
  cancellation, multi-worker behavior, and VM ownership cleanup.

## How To Use This Index

1. Read [index.md](index.md).
2. Pick one task path from the reading orders below.
3. Avoid historical docs unless you need rationale or earlier design intent.

## Current Source-of-Truth Docs

- [index.md](index.md): this routing document; read first and return here when
  the task changes.
- [whitepaper.md](whitepaper.md): closest thing to a project-definition doc;
  open when you need the language/runtime thesis, target workloads, or major
  design goals.
- [numerics.md](numerics.md): core numeric representation and width-specific
  module policy; open before changing numeric types, operators, conversions, or
  arithmetic builtins.
- [runtime.md](runtime.md): current isolated-process runtime model, async I/O
  suspension, scheduler/process ownership, and structured concurrency surface;
  open for runtime, scheduler, spawn/await, or cancellation work.
- [vm.md](vm.md): current VM ownership boundaries, runtime/VM contract, process
  creation path, and measured process-creation cost; open for VM, spawn,
  cancellation, or allocation work.
- [error-handling.md](error-handling.md): current policy for recoverable errors,
  library-owned error types, and layering; open before changing public error
  surfaces or stdlib conventions.
- [modules.md](modules.md): current module-system status and explicit non-goals;
  open before changing `signature`, `structure`, `:`, `:>`, or qualified-name
  behavior.

## Implementation / Architecture Docs

- [system.md](system.md): repo-wide crate map and pipeline snapshot; useful for
  fast orientation and locating code, but treat line counts and inventory totals
  as snapshot data, not invariants.
- [rust-memory-layout.md](rust-memory-layout.md): Rust inline layout, enum size,
  and boxing guidance for hot VM data structures; open before changing
  `HeapObject`, process table state, or other high-cardinality runtime types.

## Middleware / Feature Docs

- [builtins.md](builtins.md): builtin surface reference grouped by domain; open
  when adding, removing, renaming, or documenting builtins.
- [builtin-domains.md](builtin-domains.md): builtin architecture, domain
  ownership, Rust path conventions, and compile/runtime gating policy; open
  before adding builtin domains or moving builtin implementation files.
- [parquet.md](parquet.md): proposed Parquet API/capability design; open only
  for Parquet work or to confirm that the feature is still design-only.

## Performance / Benchmarking Docs

- [vm.md](vm.md): the only current docs page with concrete performance guidance;
  use it for process-creation cost, allocation count, and the rerunnable
  benchmark command.

## Verification / Testing Docs

- [verification.md](verification.md): current bytecode verifier guarantees,
  non-guarantees, formal-spec inventory, and verification roadmap; open first
  for verifier, fuzzing, model-checking, deadlock, wakeup, cancellation, or
  scheduler-correctness work.
- [verification-status-20260428.md](verification-status-20260428.md): dated
  verification grade snapshot and reassessment prompt; use as historical context
  when updating verification priorities.
- [verification-tla.md](verification-tla.md): older focused status page for the
  TLA+/Quint specs; useful for extra detail, but prefer
  [verification.md](verification.md) for the current overview.

## Migration / User Guidance Docs

- [sml-deltas.md](sml-deltas.md): Hiko vs SML policy document; open when porting
  SML assumptions, changing semantics, or deciding whether to preserve or break
  legacy ML behavior.

## Historical / Superseded / Journal-Style Docs

- [bootstrap.md](bootstrap.md): original bootstrap and staged-language roadmap;
  useful for early design intent and feature sequencing history, not for
  current implementation details.
- [stuctured-concurrency.md](stuctured-concurrency.md): early structured
  concurrency/cancellation design note; conceptual, partly superseded by
  [runtime.md](runtime.md), and the filename is misspelled.

## Review Notes / External Analysis Docs

- None currently under `docs/`.

## Visual Assets / SVGs

- None currently under `docs/`.

## Supporting Non-Markdown Files

- [hiko.ebnf](hiko.ebnf): grammar artifact; open only for parser/grammar work.
- [full-builtin-run-config.example.toml](full-builtin-run-config.example.toml):
  example run-policy config; open for capability/configuration work.

## Recommended Reading Order

### Understanding The Product

1. [README.md](../README.md)
2. [whitepaper.md](whitepaper.md)
3. [system.md](system.md)
4. [runtime.md](runtime.md)
5. [modules.md](modules.md) or [builtins.md](builtins.md), depending on focus

### Changing Public API

1. [README.md](../README.md)
2. [whitepaper.md](whitepaper.md)
3. [numerics.md](numerics.md) if the change affects numeric behavior
4. [error-handling.md](error-handling.md)
5. [modules.md](modules.md) and/or [builtins.md](builtins.md)
6. [sml-deltas.md](sml-deltas.md) if the change affects language behavior

### Performance Work

1. [vm.md](vm.md)
2. [runtime.md](runtime.md)
3. [rust-memory-layout.md](rust-memory-layout.md) if changing hot Rust data structures
4. [system.md](system.md)
5. Open code after you know whether the hot path is VM-local or runtime-wide

### Correctness / Verification Work

1. [runtime.md](runtime.md)
2. [vm.md](vm.md)
3. [verification.md](verification.md)
4. [verification-tla.md](verification-tla.md) if you need the older focused TLA+/Quint status page
5. [sml-deltas.md](sml-deltas.md) if semantics or language rules are involved
6. Prefer `specs/tla` over `specs/quint` if you need the current formal model

### Middleware Work

1. [builtins.md](builtins.md)
2. [builtin-domains.md](builtin-domains.md)
3. [error-handling.md](error-handling.md)
4. [runtime.md](runtime.md) if the middleware suspends, spawns, or crosses
   process boundaries
5. [full-builtin-run-config.example.toml](full-builtin-run-config.example.toml)
   if capabilities/config are involved

### Migration / Adoption Work

1. [README.md](../README.md)
2. [sml-deltas.md](sml-deltas.md)
3. [modules.md](modules.md)
4. [builtins.md](builtins.md)
5. [error-handling.md](error-handling.md)

## Default Avoid Unless Needed

- Skip [bootstrap.md](bootstrap.md) for day-to-day implementation work.
- Skip [stuctured-concurrency.md](stuctured-concurrency.md) unless you are
  tracing historical design intent.
- Skip [parquet.md](parquet.md) unless you are working specifically on Parquet.
- Use [system.md](system.md) for orientation, not for exact counts.
- Use [whitepaper.md](whitepaper.md) for intent and positioning, not as a
  substitute for current runtime/VM contracts.
