# Hiko vs SML

Hiko is **SML-derived, not SML-obedient**.

Its core semantics are rooted in Core SML, but Hiko does not aim to reproduce every historical quirk, ambiguity, or under-specified corner of SML'97. This document tracks the most important known SML defect clusters and triages them for Hiko.

The goal is practical:

- keep the parts of SML that are strong foundations
- simplify or omit areas that are historically messy
- document deliberate divergences instead of drifting into them accidentally

This is a design note, not a complete catalog of all issues in the SML definition. For the underlying source material, see Rossberg's defect list and HaMLet/HaMLet S.

## Defects We Already Avoided

| SML defect | Why it is avoided in Hiko | Practical implication |
|---|---|---|
| `val rec` weirdness and static/dynamic mismatch | Hiko chose the simpler recursive-binding surface | Keep only the simplified rule; do not reintroduce legacy `val rec` forms |
| `abstype` incoherence | Hiko does not have `abstype` | No need to model its equality and principality problems |
| Large parts of SML's grammar ambiguity | Hiko's surface is much smaller than full SML | Keep specifying one parse, not “whatever the implementation does” |
| Module-system complexity from full SML | Hiko stopped at minimal modules: `structure`, `signature`, `:>`, qualified names | Do not rush into full SML module features |
| Confusion around uppercase primitive type names vs ML conventions | Hiko normalized source syntax to lowercase builtin types | Keep source syntax ML-like and explicit |

## Defects We Still Need To Guard Against

| SML defect | Why it still matters for Hiko | Practical implication |
|---|---|---|
| Under-specified defaulting machinery | Hiko already rejects operator overloading; the remaining risk is accidentally reintroducing ad hoc defaulting through literals or builtin families | Keep operators monomorphic and require any future defaulting rule to be explicit and formal |
| Parser ambiguity from new sugar | Every new convenience feature can reintroduce SML-style ambiguity | For each syntax feature, specify precedence and a unique parse up front |
| Exhaustiveness and redundancy edge cases | Hiko already performs pattern analysis; richer patterns make it subtle quickly | Keep pattern features modest unless the checker story stays precise |
| Module elaboration complexity creep | Adding functors, `where type`, local modules, or first-class modules would import the hard part of SML | Treat new module features as expensive and require a concrete use case; in particular, do not add functors just to get `MakeSet`/`MakeMap`, because Hiko can cover that use case with ordinary modules plus explicit comparator-passing |
| Toplevel and type-inference corner cases | HM inference plus declaration forms always has sharp edges | Keep the implementation explicit and regression-test every deliberate divergence |
| Executable-spec drift | SML and HaMLet show how easily parser, typechecker, and implementation behavior can drift apart | Keep docs, semantics, and tests aligned |

## Defects That Are Irrelevant Because Hiko Intentionally Diverges

| SML defect | Why it is irrelevant for Hiko | Practical implication |
|---|---|---|
| Full Basis overloading complexity (`word`, `real`, etc.) | Hiko is not trying to replicate the full SML Basis surface | Do not import that hierarchy unless it is clearly needed |
| Flexible-record under-specification | Hiko does not currently have SML-style records and row inference | Ignore for now; revisit only if records become a real feature |
| Sharing, `where type`, and manifest-type oddities | Hiko intentionally omitted those advanced module corners | Keep them out unless there is a strong need |
| Higher-order functor principal-typing problems | Hiko does not have higher-order functors | Keep it that way unless there is a concrete reason to pay that complexity; generic collections are not a sufficient reason on their own |
| Views, transformation patterns, and first-class modules | Those are Successor ML experiments, not part of Hiko's core direction | Treat them as out of scope |
| `use`/REPL/Basis-loading oddities from SML | Hiko has a different config, runtime, and module-delivery story | Build explicit import and config rules instead of inheriting historical SML behavior |
| Mailbox-style language baggage | Hiko already moved to `spawn` / `await_process` and runtime-managed suspension | Keep message passing out unless intentionally reintroduced |

## Design Policy

When Hiko overlaps with SML in an area that is known to be historically messy:

1. treat the ambiguity or under-specification as a bug, not as tradition
2. choose the smallest rule that supports Hiko's goals
3. document the choice explicitly
4. add parser, typechecker, or runtime tests for the chosen behavior

That policy matters more than whether Hiko tracks any particular “Successor ML” proposal.

### On Toplevel and Type Inference

Hiko should treat HM inference as a strength, but not as something to stretch aggressively.

In particular, Hiko should avoid **heroic inference**: implementation tricks that try very hard to infer a type in awkward corner cases by adding special-case reasoning, hidden top-level rules, or inference behavior that is difficult to explain from the language surface.

Examples of what to avoid:

- special top-level generalization rules that differ from local scope without a clear language-level reason
- ad hoc recovery heuristics for recursive bindings
- context-sensitive inference tricks that silently change behavior depending on declaration ordering
- inference extensions whose behavior is hard to state simply in the language docs

The preferred policy is:

- keep inference conservative
- prefer explicit rules over clever recovery
- keep the value restriction simple
- reject unclear cases with a precise error instead of trying to guess what the programmer meant
- add regression tests whenever generalization, recursion, abstract types, or declaration forms are changed

This is especially important because Hiko already combines HM inference with modules, opaque abstraction, effects, and a non-trivial runtime model. Simplicity in the inference story is worth preserving.

## References

- Andreas Rossberg, *Defects in the Revised Definition of Standard ML*:
  <https://people.mpi-sws.org/~rossberg/papers/sml-defects-2013-09-18.pdf>
- HaMLet:
  <https://people.mpi-sws.org/~rossberg/hamlet/>
- HaMLet S:
  <https://people.mpi-sws.org/~rossberg/hamlet/hamlet-succ-1.3.2S6.pdf>
- SML Family / Successor ML:
  <https://smlfamily.github.io/>
  <https://smlfamily.github.io/successor-ml/>
