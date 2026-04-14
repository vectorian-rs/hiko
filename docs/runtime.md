
# Hiko Runtime: Effect-Based Async with Isolated Processes

## Overview

Hiko's runtime executes many **isolated processes** on a fixed pool of worker threads. Each process has its own VM, heap, stack, and effect handlers. No mutable state is shared between processes.

The system exposes **effect-based async I/O with structured concurrency (`spawn` / `await`)**, without function coloring.

This design combines:

* **Algebraic effects** for direct-style async
* **Process isolation** for safety and simple GC
* **Per-process heaps and GC**
* **Arc-shared immutable leaves** for efficient data transfer at process boundaries

---

## Comparison with alternatives

|                       | Hiko                           | OCaml Eio              | Erlang/BEAM              |
| --------------------- | ------------------------------ | ---------------------- | ------------------------ |
| Concurrency unit      | Isolated process (own heap)    | Fiber (shared heap)    | Process (own heap)       |
| Communication model   | Structured (`spawn` / `await`) | Shared memory          | Message passing          |
| GC                    | Per-process, independent       | Global, stop-the-world | Per-process, independent |
| Async style           | Effects (no coloring)          | Effects (no coloring)  | Receive loops            |
| Continuations         | Local to process               | Local to fiber         | None                     |
| Large payload passing | Zero-copy via `Arc<str>`       | Zero-copy (same heap)  | Deep copy                |

We borrow from Eio: direct-style async via effects.
We borrow from Erlang: isolated processes and per-process GC.
We **do not adopt the actor/message-passing programming model**.

---

## Architecture

```
⟨ Processes, Scheduler, IoBackend ⟩
```

Processes are scheduled onto a pool of worker threads. A process may move between threads; ownership of heap and state is always with the process.

---

## Process

```rust
struct Process {
    pid: Pid,
    vm: VM,
    status: ProcessStatus,
    parent: Option<Pid>,
    result: Option<SendableValue>,
    blocked_continuation: Option<GcRef>, // GC root when suspended
}
```

Each process owns:

* VM (heap, stack, frames, handlers)
* its continuation when suspended
* its execution lifecycle

There is **no mailbox** and no general inter-process messaging in the user model.

---

## Process boundaries and values

### SendableValue

Values that cross process boundaries (e.g., spawn captures, await results):

```rust
enum SendableValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    Tuple(Vec<SendableValue>),
    List(Vec<SendableValue>),
    Data { tag: u16, fields: Vec<SendableValue> },
}
```

Rule:

> **Copy the shape, share immutable leaves**

---

## Effects

Effects are **process-local**, with extended resolution in `perform`.

### Resolution order

1. User handler
2. Runtime-handled effect
3. Unhandled effect error

No new surface syntax is introduced.

---

## Runtime-handled effects (I/O)

Certain effects are intercepted by the runtime when no user handler is present.

### Example

```sml
effect HttpGet of String

val res = perform HttpGet "https://api.example.com"
```

### Execution

1. No user handler found

2. Effect matches runtime-handled table

3. VM:

   * captures continuation
   * stores it in `blocked_continuation`
   * returns `RuntimeEffect { tag, payload }`

4. Scheduler:

   * marks process `Blocked(Io)`
   * submits request to I/O backend

5. On completion:

   * result is deserialized into process heap
   * continuation is resumed
   * process is re-enqueued

---

## I/O semantics

### One-shot continuations

* Continuations are resumed **exactly once**
* No multi-shot continuations in v1

### Result-based error handling

All runtime-handled I/O effects return:

```sml
datatype 'a result = Ok of 'a | Err of Error
```

Usage:

```sml
val res = perform HttpGet url

case res of
  Ok body => ...
| Err e   => ...
```

### Rule

> The runtime never kills a process for I/O failure. It always resumes with a `Result`.

---

## Scheduler

Scheduler is abstracted via a trait:

```rust
trait Scheduler {
    fn enqueue(&self, pid: Pid);
    fn dequeue(&self) -> Option<Pid>;
    fn remove(&self, pid: Pid);
    fn reductions(&self, pid: Pid) -> u64;
}
```

### Worker loop

```
loop:
  pid = dequeue()
  run process for N reductions

  match result:
    Yield         → enqueue
    Done          → complete + wake awaiters
    RuntimeEffect → block + register I/O
    Await         → block or resume
```

The scheduler only observes:

* Yield (fuel exhaustion)
* Block (I/O, await)
* Done

---

## Garbage collection

### Per-process

* Each process has its own heap
* GC pauses only that process
* No global stop-the-world

### Root set

Includes:

* stack
* frames
* handlers
* **blocked_continuation**

### Invariant

> **No pointer from process A's heap into process B's heap**

---

## Structured concurrency

### API

```sml
val pid = spawn (fn () => perform HttpGet url)
val res = await pid
```

### Semantics

* `spawn` creates a new isolated process
* `await` blocks until completion
* result is delivered exactly once

---

## Semantic decisions (v1)

### Await

* Only parent may await child
* Result consumed once

### Spawn

* Closures are serialized via captured values
* Non-sendable captures → runtime error

---

## Failure model

| Case                        | Behavior                 |
| --------------------------- | ------------------------ |
| Child process failure       | propagated via `await`   |
| I/O failure                 | returned as `Result.Err` |
| Runtime invariant violation | process failure          |

---

## Safety

### Enforced by Rust

* No shared mutable state
* `SendableValue` cannot contain `GcRef`
* `Arc` ensures safe immutable sharing

### Cannot happen

* Data races
* Cross-process pointer corruption
* Concurrent GC bugs

---

## Design principles

1. Isolation over sharing
2. Copy structure, share immutable data
3. Effects are control flow, not I/O
4. Runtime interprets effects as I/O
5. GC is local
6. Structured concurrency over unstructured messaging

---

## Summary

> Hiko provides effect-based asynchronous programming with isolated processes, enabling direct-style concurrency without shared memory, function coloring, or complex global GC.

