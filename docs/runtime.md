# Hiko Runtime: Direct-Style Async with Isolated Processes

## Overview

Hiko's runtime executes many **isolated processes** on a fixed pool of worker threads. Each process has its own VM, heap, stack, and effect handlers. No mutable state is shared between processes.

The system exposes **runtime-managed async I/O with structured concurrency**, without function coloring.

This design combines:

- **Algebraic effects** for process-local control flow
- **Process isolation** for safety and simple GC
- **Per-process heaps and GC**
- **Arc-shared immutable leaves** for efficient data transfer at process boundaries

## Comparison with alternatives

|                       | Hiko                           | OCaml Eio              | Erlang/BEAM              |
| --------------------- | ------------------------------ | ---------------------- | ------------------------ |
| Concurrency unit      | Isolated process (own heap)    | Fiber (shared heap)    | Process (own heap)       |
| Communication model   | Structured (`spawn` / `await`) | Shared memory          | Message passing          |
| GC                    | Per-process, independent       | Global, stop-the-world | Per-process, independent |
| Async style           | Runtime-managed suspension     | Effects (no coloring)  | Receive loops            |
| Continuations         | Local to process               | Local to fiber         | None                     |
| Large payload passing | Zero-copy via `Arc<str>`       | Zero-copy (same heap)  | Deep copy                |

We borrow from Eio: direct-style async ergonomics.
We borrow from Erlang: isolated processes and per-process GC.
We **do not adopt the actor/message-passing programming model**.

## Architecture

```
⟨ Processes, Scheduler, IoBackend ⟩
```

Processes are scheduled onto a pool of worker threads. A process may move between threads; ownership of heap and state is always with the process.

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

- VM (heap, stack, frames, handlers)
- its continuation when suspended
- its execution lifecycle

There is **no mailbox** and no general inter-process messaging.

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

Effects are **process-local** and used for **user-defined control flow** (state, generators, error handling).

Effects are NOT the I/O surface. I/O uses builtins/stdlib functions.

---

## I/O Model

I/O operations are exposed as ordinary builtins:

```sml
val (status, headers, body) = http_get "https://api.example.com"
val _ = sleep 1000
val content = read_file "data.txt"
```

The same source code works in both runtimes:

- **Single-threaded runtime**: builtins block the thread
- **Threaded runtime**: builtins suspend the process via `RuntimeRequest::Io`, the I/O backend handles the operation, and the process resumes when complete

No function coloring. No `perform`. No effect declarations for I/O.

### Testing

Use a mock I/O backend (`MockIoBackend`) for deterministic testing. The backend is pluggable at the runtime level, not at the language level.

---

## I/O semantics

### One-shot continuations

- Continuations are resumed **exactly once**
- No multi-shot continuations in v1

### Error handling

Builtin and library APIs should model recoverable failure with `Std.Result`:

```sml
val loaded =
  Json.parse text
  |> Result.map_err Config.Parse
```

`Std.Fiber.join` follows the same rule:

```sml
case Fiber.join child of
    Result.Ok value => ...
  | Result.Err err => println (Fiber.render_error err)
```

Joining a child no longer fails the parent process. Process-level outcomes such
as cancellation, fuel exhaustion, heap exhaustion, and runtime failure are
returned as `Fiber.error`.

See [error-handling.md](error-handling.md) for the standard library/application error-layering pattern.

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
    Io            → block + register I/O
    Await         → block or resume
```

The scheduler only observes:

- Yield (fuel exhaustion)
- Block (I/O, await)
- Done

---

## Garbage collection

### Per-process

- Each process has its own heap
- GC pauses only that process
- No global stop-the-world

### Root set

Includes:

- stack
- frames
- handlers
- **blocked_continuation**

### Invariant

> **No pointer from process A's heap into process B's heap**

---

## Structured concurrency

### API

The raw runtime builtins are:

```sml
val pid = spawn (fn () => http_get url)
val winner = wait_any [pid]
val _ = cancel pid
val res = await_process pid
```

The intended user-facing layer is `Std.Fiber`:

```sml
import Std.Fiber

val fast =
  Fiber.first (
  (fn () => http_get url_a),
  (fn () => http_get url_b)
)

case fast of
    Result.Ok response => response
  | Result.Err err => panic (Fiber.render_error err)
```

Cancellation is cooperative:

- `Fiber.cancel` is fire-and-forget
- the child observes it at the next suspension point
- cancelling an already-finished child is a no-op
- `Fiber.join` on a cancelled child returns `Result.Err ...`

### Semantics

- `spawn` creates a new isolated process
- `join`/`await_process` block until completion
- result is delivered exactly once

---

## Semantic decisions (v1)

### Await

- Only parent may await child
- Result consumed once

### Spawn

- Closures are serialized via captured values
- Non-sendable captures → runtime error

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

- No shared mutable state
- `SendableValue` cannot contain `GcRef`
- `Arc` ensures safe immutable sharing

### Cannot happen

- Data races
- Cross-process pointer corruption
- Concurrent GC bugs

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
