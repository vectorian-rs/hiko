# Hiko Documentation Index

Read this file first. It is a routing table for agents and maintainers, not a replacement for the topic docs.

If you need repo-wide product context before loading docs, skim [README.md](../README.md). There is no separate PRD in this repo; the closest design-intent document is [project/whitepaper.md](project/whitepaper.md).

Review snapshot: 2026-05-26.

## Documentation map

| Area                        | Folder                                   | Status                                                         | What we have                                                                                             | Start here                                                                                                                                                               |
| --------------------------- | ---------------------------------------- | -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Project intent and workflow | [`project/`](project/index.md)           | Mixed: current standards plus historical bootstrap             | Whitepaper, development standard, documentation guidance, early bootstrap, evaluation prompt             | [`project/whitepaper.md`](project/whitepaper.md), then [`project/dev-standard.md`](project/dev-standard.md)                                                              |
| Language semantics          | [`language/`](language/index.md)         | Current policy docs plus draft spec and grammar artifact       | Draft definition, effects, errors, modules, numeric policy, SML deltas, EBNF grammar                    | [`language/definition.md`](language/definition.md), then [`language/modules.md`](language/modules.md) or [`language/effects.md`](language/effects.md), depending on task |
| Runtime and VM architecture | [`architecture/`](architecture/index.md) | Current architecture docs plus one historical note             | System snapshot, runtime/process model, VM/runtime seam, Rust memory layout, structured concurrency note | [`architecture/runtime.md`](architecture/runtime.md) and [`architecture/vm.md`](architecture/vm.md)                                                                      |
| Builtins and capabilities   | [`builtins/`](builtins/index.md)         | Current reference plus design proposals                        | Builtin reference, builtin-domain gating, full config example, Parquet proposal                          | [`builtins/builtins.md`](builtins/builtins.md), then [`builtins/builtin-domains.md`](builtins/builtin-domains.md)                                                        |
| Verification and testing    | [`verification/`](verification/index.md) | Current overview plus older/dater snapshots                    | Bytecode verifier status, formal spec inventory, TLA+/Quint details, dated grade snapshot                | [`verification/verification.md`](verification/verification.md)                                                                                                           |
| Agent prompts               | [`prompts/`](prompts/index.md)           | Operational prompts                                            | Code simplifier and review prompts                                                                       | [`prompts/index.md`](prompts/index.md)                                                                                                                                   |
| Research and experiments    | [`rnd/`](rnd/index.md)                   | Experimental/design-only                                       | Infrastructure-as-code demo                                                                              | [`rnd/iac-demo.md`](rnd/iac-demo.md)                                                                                                                                     |

## Status legend

- **Current source of truth**: use before changing behavior in that area.
- **Snapshot**: useful for orientation, but counts and inventory may drift from code.
- **Historical**: use for rationale only; prefer current docs for implementation.
- **Proposal / design-only**: not necessarily implemented.

## Recommended reading routes

### Understanding the project

1. [README.md](../README.md)
2. [project/whitepaper.md](project/whitepaper.md)
3. [architecture/system.md](architecture/system.md)
4. [architecture/runtime.md](architecture/runtime.md)
5. [architecture/vm.md](architecture/vm.md)

### Changing public language or stdlib behavior

1. [project/whitepaper.md](project/whitepaper.md)
2. [language/modules.md](language/modules.md), [language/effects.md](language/effects.md), or [language/numerics.md](language/numerics.md)
3. [language/error-handling.md](language/error-handling.md)
4. [builtins/builtins.md](builtins/builtins.md) if the surface includes builtins
5. [language/sml-deltas.md](language/sml-deltas.md) if the change touches SML compatibility or deliberate divergence

### Runtime, VM, scheduler, or cancellation work

1. [architecture/runtime.md](architecture/runtime.md)
2. [architecture/vm.md](architecture/vm.md)
3. [architecture/rust-memory-layout.md](architecture/rust-memory-layout.md) for hot data structures
4. [verification/verification.md](verification/verification.md)
5. [verification/verification-tla.md](verification/verification-tla.md) for formal-model detail

### Builtin, capability, or run-config work

1. [builtins/builtins.md](builtins/builtins.md)
2. [builtins/builtin-domains.md](builtins/builtin-domains.md)
3. [builtins/full-builtin-run-config.example.toml](builtins/full-builtin-run-config.example.toml)
4. [language/error-handling.md](language/error-handling.md)
5. [architecture/runtime.md](architecture/runtime.md) if the builtin suspends, spawns, performs I/O, or crosses process boundaries

### Verification or correctness work

1. [verification/verification.md](verification/verification.md)
2. [architecture/runtime.md](architecture/runtime.md)
3. [architecture/vm.md](architecture/vm.md)
4. [verification/verification-tla.md](verification/verification-tla.md)
5. Formal models under [`../specs/`](../specs/) when implementation behavior and prose disagree

## Current documentation gaps

- There is no complete formal language definition yet. [`language/definition.md`](language/definition.md) is the short first draft and entry point; see [project/how-to-document-hiko.md](project/how-to-document-hiko.md) for the documentation strategy.
- [architecture/system.md](architecture/system.md) is a useful architecture snapshot, but its exact counts should be treated as non-authoritative.
- [project/bootstrap.md](project/bootstrap.md), [architecture/structured-concurrency.md](architecture/structured-concurrency.md), and dated verification snapshots are historical context, not primary implementation contracts.
- [builtins/parquet.md](builtins/parquet.md) and [`rnd/`](rnd/index.md) are proposal/R&D material unless implementation work explicitly says otherwise.

## Maintenance rule

When adding a doc, place it in the nearest topic folder and update that folder's `index.md`. If a behavior change affects public language, runtime, builtin, or verification semantics, update the relevant source-of-truth doc in the same change.
