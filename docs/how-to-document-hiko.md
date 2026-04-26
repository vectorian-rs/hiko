# How to Document Hiko

Hiko needs two complementary documentation tracks:

1. **Whitepaper**: design rationale, orientation, and motivation.
2. **Definition-style specification**: precise syntax, semantics, and runtime behavior.

Keep the whitepaper narrative-focused. Put formal phrase classes, judgment forms, semantic objects, and derived forms in a separate specification document.

## Recommended specification structure

Use a Definition-style document such as `docs/definition.md`, or split it by topic when it grows:

- `docs/definition-core.md`
- `docs/definition-runtime.md`
- `docs/definition-modules.md`

A useful initial outline is:

1. Introduction
2. Syntax of the Core
3. Syntax of Modules
4. Static Semantics of the Core
5. Static Semantics of Modules
6. Dynamic Semantics of the Core
7. Dynamic Semantics of Effects
8. Dynamic Semantics of Processes and Runtime
9. Programs
10. Appendices

Appendices should cover:

- derived forms
- full grammar
- initial basis and builtin surface
- current, intended, and omitted features

## Hiko-specific additions

Unlike the SML Definition, Hiko needs a formal VM/runtime seam. Document these runtime concepts explicitly:

- process lifecycle
- `spawn`, `await`, `wait_any`, and cancellation
- capability boundaries
- sendable-value transfer
- runtime states such as `Done`, `Yielded`, `Failed`, `Spawn`, `Await`, `AwaitResult`, `Cancel`, `WaitAny`, `Io`, and `Cancelled`

## Effects and mathematics

Hiko has mathematically definable effect semantics, but it does not yet have a full static type-and-effect system. Avoid calling it an academic "effect system" unless that distinction is clear.

The formal effects section should define:

- algebraic operations, such as `perform Op v`
- handlers, such as `handle e with ...`
- one-shot captured continuations
- operational reduction by capturing the evaluation context up to the nearest handler
- the boundary between local effect semantics and VM scheduling

Important effect-semantics questions include:

- which continuation segment is captured
- where handler search stops
- whether continuations are one-shot or multi-shot
- whether effects are local or can cross process boundaries
- what `resume` means after a continuation has already been resumed
- where effect semantics stops and runtime scheduling begins

## Suggested formal judgments

A first specification can introduce judgments like:

```text
Gamma |- e : tau
Gamma |- pat : tau => Gamma'
B |- spec => I
E |- e ⇓ v
P, R, W -> P', R', W'
```

Use these to separate:

- core typing
- pattern typing
- module and signature elaboration
- local dynamic semantics
- effect handling
- runtime transitions over process tables, waiters, tombstones, and I/O registrations

## Key design distinction

Effects are local control semantics. Async is runtime transition semantics.

Do not model async as effect interpretation. Model it as a labeled transition system at the VM/runtime layer.
