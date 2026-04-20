# Local Algebraic Effects on an Isolated-Process Runtime

## Abstract

Asynchronous programming is often split between two unsatisfying approaches. Future- and callback-based systems impose function coloring and make direct-style code harder to preserve. Shared-heap effect runtimes recover direct style, but push substantial complexity into scheduling, memory management, and synchronization. Hiko explores a different point in the design space: local algebraic effects for intra-process control flow, combined with an isolated-process runtime with per-process heaps, explicit process-boundary transfer, and host-provided capabilities.

The contribution is not the claim that algebraic effects in isolation are novel outside a shared heap. The interesting claim is the combination: local effects stay process-local, asynchronous I/O is mediated by a fixed host runtime protocol, heaps remain isolated per process, and capabilities stay host-owned. This produces direct-style scripting inside each process, explicit ownership boundaries between processes, local garbage collection, and a runtime model that is easier to reason about for safety-oriented tooling.

## 1. Introduction

Hiko is aimed at scripting and tooling workloads where safety, predictability, and efficiency matter more than user-programmable concurrency semantics. Typical workloads include reading files and HTTP resources, invoking external tools without a shell, orchestrating document pipelines, and eventually integrating native data capabilities such as cloud APIs, databases, or columnar file readers.

These workloads want direct-style programming, but they also want strong control over authority and resource access. In many existing systems, direct-style asynchronous programming is achieved through a shared-heap concurrency substrate. Hiko instead asks whether direct style can be preserved while keeping scheduling, capability enforcement, and heap ownership fixed in the runtime.

Hiko is rooted in the SML tradition, but it is not intended as a bug-for-bug reproduction of SML'97. Where the SML specification is historically ambiguous, under-specified, or unnecessarily complex for Hiko's goals, Hiko prefers explicit simplification. Examples include a simplified recursive-binding surface, omission of `abstype`, a deliberately minimal module system, lowercase primitive type names in source syntax, a small number of explicit conveniences such as the `|>` pipeline operator, and a general preference for one documented parse over inherited grammar ambiguity.

The answer proposed here is yes. Hiko combines:

- a Core SML-style language foundation
- local algebraic effects and one-shot continuations
- isolated processes with per-process heaps
- explicit process-boundary transfer via `SendableValue`
- runtime-managed suspension for asynchronous operations
- host-provided, policy-controlled capabilities

The result is not a programmable scheduler or a general-purpose user-defined concurrency framework. It is a fixed runtime model designed for safe orchestration of external work.

The research claim should therefore be read as a compositional one: Hiko is interesting because of how these ingredients are arranged together, not because any single ingredient is independently unprecedented.

### Contributions

- A language/runtime design that separates local algebraic effects from runtime-managed asynchronous suspension while keeping capability enforcement and scheduling in the host runtime.
- An isolated-process execution model with per-process heaps and local garbage collection.
- A process-boundary transfer model based on `SendableValue`, including shared immutable leaf payloads for strings and bytes.
- A capability-oriented runtime architecture suitable for safe tooling and agentic coding workloads.
- A layered formalization strategy that extends Core SML semantics with local effects and an isolated-process runtime model.

## 2. Design Goals

The design is driven by a small set of explicit goals.

### 2.1 Safety

External authority should remain in the host runtime. Filesystem access, HTTP, process execution, and future native integrations should be exposed as policy-controlled capabilities rather than as user-definable runtime behavior.

### 2.2 Direct Style

Scripts should be able to express multi-step workflows in direct style without forcing every function into a future- or callback-passing discipline.

### 2.3 Runtime Predictability

Scheduling and suspension semantics should be fixed and auditable. User code may express local control abstractions through effects, but it should not redefine the scheduler or capability semantics.

### 2.4 Many-Core-Friendly Execution

Per-process heaps and local garbage collection should avoid a shared global heap bottleneck and allow process lifetimes to act as natural memory reclamation boundaries.

### 2.5 Host Extensibility

The runtime should be easy to extend with native capabilities such as process execution, cloud APIs, databases, parquet readers, or other host integrations without changing the guest concurrency model.

## 3. Background

### 3.1 Core SML

Hiko is rooted in Core SML: a typed lambda-calculus core with Hindley-Milner polymorphism, algebraic data types, and formal static and dynamic semantics. This provides a strong basis for both implementation and future formal reasoning.

However, Hiko should be understood as **SML-derived, not SML-obedient**. The point of using Core SML as a foundation is to inherit a strong semantic base, not to inherit every historical defect or specification gray area of SML'97. In practice, this means Hiko keeps the robust parts of the core language while explicitly repairing, simplifying, or omitting features that are known to be awkward in the SML definition.

### 3.2 Algebraic Effects

Algebraic effects provide `perform`, `handle`, and `resume` as structured control-flow operators. In Hiko, their role is intentionally local. Effects are for intra-process abstractions such as error handling, state, generators, and other control patterns that can be expressed through one-shot continuations.

### 3.3 Shared-Heap Effect Systems

Shared-heap systems demonstrate that effect handlers can support direct-style asynchronous programming. They also highlight the cost of letting concurrency and scheduling semantics live in the same heap and continuation space as ordinary language execution.

### 3.4 Isolated-Process Runtimes

Isolated-process execution offers a different tradeoff: explicit boundaries, local heaps, and local GC in exchange for explicit transfer across process boundaries. Hiko adopts this tradeoff because it fits safety-oriented scripting better than unconstrained sharing.

## 4. System Model

The system is organized around isolated processes.

Each process owns:

- its own VM instance
- its own heap
- its own operand stack and call frames
- its own handler stack and local continuations
- optional parent/child linkage and runtime status

The runtime owns:

- the process table
- the scheduler
- the I/O backend
- capability policies and host integration points

Compiled program data is immutable and may be shared across processes. Mutable execution state is not.

## 5. Local Effects

The effect system is process-local by design.

### 5.1 What Effects Are For

Effects exist to express structured intra-process control flow. Good fits include:

- local error handling
- state threading
- generator-like control patterns
- interpreters and evaluators
- local instrumentation or context propagation

### 5.2 What Effects Are Not For

Effects are not used to define the scheduler, to reimplement I/O semantics, or to bypass host capability enforcement. This is a deliberate design choice. Hiko is a scripting language with a fixed runtime model, not a platform for user-defined concurrency runtimes.

### 5.3 One-Shot Continuations

Continuations are one-shot. This keeps the implementation simpler, aligns with efficient runtime suspension, and fits the intended workload better than unrestricted multi-shot continuations.

## 6. Runtime Suspension and I/O

Asynchronous I/O in Hiko is runtime-driven rather than user-effect-driven.

When a capability-bearing builtin would block, the VM emits a runtime suspension request. The runtime records the blocked process, registers the operation with an abstract backend, and resumes the process when the backend reports completion.

This design has several consequences:

- authority remains in the host runtime
- capability checks happen at the runtime boundary
- backend choice is hidden from guest code
- scheduling semantics remain fixed
- scripts still resume in direct style

This differs from designs where I/O itself is modeled as a user-visible effect. Hiko keeps local effects and runtime suspension separate because the language is intended for safe orchestration, not user-programmable scheduler construction.

## 7. Process-Boundary Values

Values crossing process boundaries are reified as `SendableValue`.

This representation exists because ordinary VM values may contain heap references that are only meaningful inside one process. `SendableValue` removes those process-local references and preserves only boundary-safe structure.

Key properties:

- no `GcRef` crosses a process boundary
- continuations do not cross process boundaries
- mutable process-local state does not cross process boundaries
- immutable string and byte leaves may share backing storage at the boundary

This is not full end-to-end zero-copy transfer. It is explicit boundary transfer with shared immutable leaves where possible.

## 8. Isolated Heaps and Garbage Collection

Each process has its own heap and collector. There is no global GC.

This has three important consequences:

- garbage collection remains local to the process that allocated the data
- allocator and collector contention are reduced
- short-lived processes can be reclaimed as whole units

Whole-process reclamation is especially important for tooling workloads. A child process may perform a bounded piece of work, return a result, and then be reaped after `await` consumes that result. This gives the system region-like behavior at process granularity without requiring a region-typed language.

## 9. Process Lifecycle and Reclamation

The runtime process lifecycle is intentionally simple:

1. spawn a child process
2. run until completion, failure, suspension, or cancellation
3. transfer the result across the process boundary if needed
4. reap the process once the result has been consumed

This matters because isolated heaps only realize their full benefit when completed processes are actually dropped promptly. Hiko now reaps finished child processes after `await_process` consumes their result, allowing the child VM and its entire heap to be reclaimed at once.

## 10. Safety and Capability Model

Hiko is designed for workloads where safety matters more than maximum runtime programmability.

The capability model is therefore host-centric:

- the host/runtime decides which external resources exist
- policies constrain filesystem, network, and process execution
- scripts compose capabilities but do not redefine them
- shell-free execution is preferred over stringly command composition

This makes the system a good fit for tooling scenarios such as document pipelines, code generation, infrastructure orchestration, and future native integrations for cloud services or structured data access.

## 11. Implementation

Hiko is implemented as a Rust bytecode VM with:

- a Core SML-style front end
- Hindley-Milner type inference
- algebraic effect handlers
- isolated processes
- single-threaded and multi-threaded runtimes

The VM owns local execution state. The runtime owns scheduling, I/O, and process management. Immutable compiled program structures are shared between parent and child processes, while heaps, stacks, and handlers remain process-local.

## 12. Evaluation

The evaluation should be aligned with the design goals rather than with a generic language shootout.

Important benchmark classes include:

- `spawn` / `await` microbenchmarks
- short-lived process workloads
- async I/O orchestration workloads
- CPU-bound parallel workloads
- large payload transfer across process boundaries

Important metrics include:

- spawn latency
- resume latency
- throughput
- memory retention
- boundary transfer cost
- reclamation behavior for short-lived processes

Comparisons should be made carefully. Async Rust is the most relevant fixed-runtime comparison point. Shared-heap effect systems such as Eio are useful as a qualitative contrast in design, not necessarily as a one-number benchmark target.

## 13. Formalization Strategy

The formal story should be explicitly layered.

### 13.1 Core Language Layer

Start from Core SML and extend it with:

- algebraic effects
- one-shot continuations
- handler semantics

This layer captures the local control-flow story.

### 13.2 Runtime Layer

Add a separate runtime semantics for:

- isolated processes
- process-boundary transfer
- runtime-driven suspension
- scheduling

This layer captures the concurrency and I/O story.

### 13.3 Key Invariants

The most important invariants to state and eventually prove are:

- process isolation
- continuation locality
- boundary safety
- scheduler safety
- GC non-interference across processes

This is where the research contribution becomes crisp: not a new primitive in isolation, but a coherent composition of known semantic ingredients into a runtime model with a different tradeoff profile.

## 14. Trade-offs and Limitations

The design is intentionally opinionated.

Advantages:

- direct-style programming within a process
- explicit ownership boundaries
- local heaps and local GC
- fixed, auditable runtime behavior
- strong fit for policy-controlled tooling workloads

Limitations:

- no arbitrary shared memory
- boundary transfer has a cost
- one-shot continuations constrain some abstractions
- scheduler semantics are not user-programmable
- some polished library abstractions still want a module system with opaque types

These are acceptable tradeoffs for the intended use case.

## 15. Future Work

Natural next steps include:

- a minimal module system with opaque abstraction
- typed process handles instead of raw integers
- richer scheduling strategies such as work stealing
- effect typing
- improved reclamation heuristics for long-lived processes
- additional host capabilities such as databases, cloud APIs, and columnar data readers

The important constraint is that these extensions should preserve the core split: local effects remain local control flow, and the runtime retains authority over suspension, capabilities, and scheduling.

## 16. Conclusion

Hiko explores a design in which algebraic effects and isolated-process execution are deliberately assigned different responsibilities. Effects provide programmable local control flow. The runtime provides fixed suspension, capability enforcement, scheduling, and process-boundary transfer. This yields a language/runtime architecture that is simpler than shared-heap effect systems, better aligned with safety-oriented tooling, and still rich enough to support direct-style orchestration of asynchronous external work.
