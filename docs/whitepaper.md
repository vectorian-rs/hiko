# 📝 Title (working)

**Algebraic Effects for Actor-Based Concurrency with Isolated Heaps**

# 📄 Abstract (≈150–200 words)

* Problem:

  * Async programming requires function coloring (JS, Rust)
  * Effect systems avoid this but rely on shared-heap runtimes (OCaml Eio)
* Insight:

  * Effects can be combined with **actor-style isolation**
* Contribution:

  * A runtime with:

    * isolated processes (per-process heap + GC)
    * algebraic effects for suspension
    * message passing with **zero-copy immutable leaves**
* Result:

  * Direct-style async without shared-state complexity
  * Competitive performance, improved large-payload messaging
* Implementation:

  * Rust VM (Hiko)
* Key claim:

  * Simpler runtime than shared-heap effect systems


# 1. Introduction

### Motivation

* Async programming complexity:

  * function coloring
  * callback / future composition
* Existing solutions:

  * Erlang → isolation, but awkward async style
  * Eio/OCaml → clean async, but complex runtime
* Gap:

  * no system combining:

    * **direct-style async**
    * **process isolation**
    * **simple runtime**

### Thesis

> Algebraic effects can provide direct-style asynchronous programming on top of an Erlang-style isolated process runtime.

### Contributions

* Effects + actors design
* Per-process VM + GC
* SendableValue model
* Arc-based zero-copy optimization
* Modular scheduler + I/O backend

# 2. Background

## 2.1 Algebraic Effects

* `perform`, `handle`, `resume`
* continuations as captured computation
* effects as structured control flow

## 2.2 Actor Model (Erlang)

* isolated processes
* message passing
* per-process GC

## 2.3 Async/Await and Function Coloring

* propagation problem
* loss of direct style

## 2.4 Effect-based Concurrency (Eio)

* fibers + shared heap
* effect handlers as schedulers


# 3. Design Overview

High-level architecture:

* runtime
* worker threads
* processes (isolated VMs)
* scheduler
* I/O backend

Key design principles:

* isolation over sharing
* effects are local control flow
* message passing boundary
* GC is local


# 4. Process Model

## 4.1 Process Structure

* VM (heap, stack, handlers)
* mailbox
* status

## 4.2 Lifecycle

* spawn
* run
* block
* resume
* terminate

## 4.3 Isolation Guarantees

* no shared mutable state
* no cross-process pointers


# 5. Effect Execution Model

## 5.1 Local Effects

* `perform` captures continuation
* handler resumes locally

## 5.2 Separation from Scheduler

* user effects vs runtime events

## 5.3 Async via Effects

* I/O as effect
* continuation capture
* resume on completion

## 5.4 One-shot Continuations

* design choice
* implications


# 6. Message Passing

## 6.1 SendableValue

Explain:

* structural copying
* immutable leaf sharing

## 6.2 Serialization Boundary

* `Value → SendableValue`
* `SendableValue → Value`

## 6.3 Zero-copy Optimization

* `Arc<str>` / `Arc<[u8]>`
* comparison with Erlang deep copy

## 6.4 Safety Invariants

* no `GcRef`
* no continuation crossing
* no mutable sharing


# 7. Scheduler

## 7.1 Model

* process = scheduling unit
* reduction counting

## 7.2 Interface (trait)

## 7.3 FIFO Scheduler (v1)

## 7.4 Future Extensions

* work stealing
* priority

## 7.5 Fairness


# 8. Garbage Collection

## 8.1 Per-process heaps

* no global GC

## 8.2 Root sets

## 8.3 Mailbox boundary

## 8.4 Interaction with Arc

* external ownership
* refcount semantics

## 8.5 Advantages vs shared heap GC


# 9. I/O Integration

## 9.1 Abstract backend

## 9.2 Readiness vs completion models

## 9.3 Effect-driven suspension

## 9.4 Resumption model


# 10. Implementation

## 10.1 Hiko VM (Rust)

## 10.2 Runtime components

* scheduler
* process table
* mailbox

## 10.3 Closure serialization

## 10.4 Key challenges

* continuation representation
* spawn semantics


# 11. Evaluation

## 11.1 Benchmarks

* message passing
* async I/O
* CPU-bound tasks

## 11.2 Comparisons

* Erlang
* async Rust
* Eio (qualitative)

## 11.3 Results

Focus especially on:

* large payload messaging
* latency
* throughput


# 12. Discussion

## Trade-offs

### Advantages

* simplicity
* isolation
* direct-style async

### Limitations

* no shared memory
* serialization overhead
* one-shot continuations
* no selective receive (v1)


# 13. Related Work

* Erlang / BEAM
* OCaml Multicore / Eio
* Eff / Koka
* async Rust / Tokio
* actor frameworks


# 14. Future Work

* work-stealing scheduler
* effect typing
* distributed processes
* selective receive
* generational GC

# 15. Conclusion

Reinforce:

> Effects + actor isolation = simpler async runtime design


# Key narrative to maintain

> We achieve the benefits of effect-based direct-style concurrency without the complexity of shared-heap runtimes.

---

## Appendix A: Research Context and Formal Foundations

### Positioning

This work is not an invention of entirely new primitives, but a **composition and extension of established research areas**:

* Standard ML (formal semantics)
* Algebraic effects and handlers
* Actor model (Erlang-style processes)
* Per-process garbage collection
* Delimited continuations

The contribution is a **coherent integration of these components into a unified runtime model**, with both implementation and formalization.

---

### Relationship to Standard ML

Standard ML provides:

* A **fully formalized mathematical semantics**

  * Static semantics (typing)
  * Dynamic semantics (evaluation)
* A **well-understood sequential core language**

This work **extends**, rather than replaces, that foundation.

Specifically, we add formal semantics for:

* Algebraic effects
* Continuations (via effect handlers)
* Concurrent processes
* Message passing
* Scheduling

Thus, the system can be viewed as:

> A formally specified concurrent extension of Standard ML with algebraic effects and actor-style execution.

---

### Formalization Goals

We aim to define a **precise operational semantics** for the extended language and runtime.

#### Core components to formalize

1. **Extended Syntax**

   * `perform E v`
   * `handle e with ...`
   * `spawn e`
   * `send e1 e2`
   * `receive ()`
   * `await e`

2. **Runtime Configuration**

A global state of the form:

```
⟨ Processes, Scheduler, Mailboxes ⟩
```

3. **Transition Relation**

A small-step relation:

```
State → State
```

capturing:

* process execution
* effect handling
* message passing
* scheduling decisions
* blocking and wakeup

---

### Key Invariants (Targets for Formal Proof)

The following properties are central to the design and are candidates for formal verification:

* **Process Isolation**

  * No pointer from process A's heap can reference process B's heap

* **Continuation Locality**

  * Continuations cannot cross process boundaries

* **Message Safety**

  * Only `SendableValue` may cross processes
  * Serialization produces no heap-local references

* **Effect Locality**

  * Effect handling and continuation resumption are process-local operations

* **Scheduler Safety**

  * No lost wakeups
  * Blocked processes are not executed
  * Runnable processes are eventually scheduled (under fairness assumptions)

* **GC Non-Interference**

  * Garbage collection in one process does not affect other processes

---

### Verification Methodology

The verification approach is **multi-layered**, combining formal proof with practical validation:

| Concern                            | Method                          | Tool             |
| ---------------------------------- | ------------------------------- | ---------------- |
| Language semantics and invariants  | Formal proof                    | Coq or Lean      |
| Scheduler correctness and liveness | Model checking                  | TLA+             |
| Concurrency bugs in implementation | Systematic interleaving testing | Loom             |
| Runtime vs model agreement         | Differential / property testing | DST / QuickCheck |

This separation reflects the strengths of each approach:

* Proof assistants ensure **mathematical correctness of the model**
* Model checking ensures **correctness under concurrency**
* Testing ensures **implementation fidelity**

---

### Scope of Formal Verification

We explicitly do **not** attempt to:

* Verify the entire Rust runtime in Coq/Lean
* Model OS-level primitives (threads, epoll, io_uring)
* Prove full system performance properties

Instead, we focus on:

> Proving that the **abstract runtime model is sound**, and validating that the implementation faithfully follows that model.

---

### Contribution

The contribution of this work is:

1. A **formally defined extension** of SML with:

   * algebraic effects
   * actor-style concurrency

2. A **runtime architecture** that:

   * preserves process isolation
   * avoids shared-heap complexity
   * supports direct-style asynchronous programming

3. A **verification strategy** that connects:

   * formal semantics
   * model checking
   * real-world implementation

---

### Summary

This work demonstrates that:

> Algebraic effects can be combined with isolated-process concurrency to produce a system that is both **formally tractable** and **practically implementable**, without requiring shared heaps or complex concurrent garbage collection.

