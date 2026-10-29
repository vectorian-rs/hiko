# Hiko: Design Outline

Hiko is a strict, statically typed, ML-family scripting language with a
bytecode VM, built for running untrusted scripts under explicit,
host-controlled capabilities.

This page states what Hiko is aiming for. Current behavior lives in the linked
documents, and [project-state.md](project-state.md) records where the
implementation stands against these aims.

## Focus

1. **Minimal language.** A small Core SML–derived language: strict evaluation,
   Hindley-Milner inference, algebraic data types, exhaustive pattern matching,
   immutable bindings, monomorphic operators, and minimal modules. Hiko is
   SML-derived, not SML-compatible.
2. **Efficient bytecode and VM.** A compact instruction set, a verifier that
   runs before execution, and a predictable interpreter with per-process heaps.
3. **Security.** Host authority stays in the host: capability-gated builtins,
   fail-closed resource limits, bytecode verification, and integrity-checked
   module loading.

**Research track: effects.** Local algebraic effect handlers stay in the
language as an experiment with open questions, not as a stable surface.

## Principles

- Prefer fewer semantic mechanisms. A reader should be able to tell what code
  does without expanding hidden dispatch.
- Recoverable failure is data (`Result`, `Option`), not exceptions.
- Effects are local to a process. The runtime, not guest code, owns
  suspension, scheduling, and capabilities.
- Heaps are isolated per process, and data crosses process boundaries only
  through explicit, typed transfer.
- Docs state what is implemented, what is intended, and what is open, and
  keep the three separate.

## Plan

**Language**

- Decide on records, mutable state (`ref`), and whether `:` ascription becomes
  transparent or is removed.
- Write a complete formal definition (static and dynamic semantics).

**Bytecode and VM**

- Renumber opcodes by group and add a versioned serialized bytecode format.
- Reduce spawn cost and `Resume` cloning, and track benchmarks over time.
- Decide the future of the threaded runtime and structured concurrency.

**Security**

- Extend the verifier toward typed bytecode.
- Add fuzz targets for the parser, type checker, verifier, policy parser, and
  `SendableValue` decoder.
- Add per-operation I/O limits and make policies less verbose.
- Bundle modules and policy into auditable artifacts.

**Effects (research)**

- Choose one-shot or multi-shot, and deep or shallow, then make the docs and
  tests agree.
- Explore effect rows in types, and capabilities as runtime-handled effects.

**Scope**

- Move the agent harness, cloud providers, and other out-of-focus tooling out
  of the repository.

## Non-goals

- Shared-memory async or a user-programmable scheduler.
- Actor-style mailboxes as the main concurrency model.
- Full SML compatibility, including functors and the full module language.
- Implicit numeric overloading or defaulting.

## Where the details are

| Topic | Document |
|---|---|
| Current state, gaps, issue triage | [project-state.md](project-state.md) |
| Repository map and pipeline | [system.md](../architecture/system.md) |
| Language | [language/index.md](../language/index.md) |
| VM and runtime | [vm.md](../architecture/vm.md), [runtime.md](../architecture/runtime.md), [execution-bounds.md](../architecture/execution-bounds.md) |
| Builtins and capabilities | [builtin-domains.md](../builtins/builtin-domains.md) |
| Verification | [verification.md](../verification/verification.md) |
| Effects | [effects.md](../language/effects.md), [effects-design.md](../rnd/effects-design.md) |
