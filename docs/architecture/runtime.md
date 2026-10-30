# Hiko Runtime: Direct-Style Async with Isolated Processes

## Overview

For the VM/runtime boundary and current process-creation cost model, see
[vm.md](vm.md).

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
    scope_id: Option<ScopeId>,
}
```

Each process owns:

- VM state (heap, stack, frames, handlers, blocked continuation root)
- lifecycle metadata (`pid`, `status`, `parent`, scope membership)

There is **no mailbox** and no general inter-process messaging.

## Process boundaries and values

### SendableValue

Values that cross process boundaries (e.g., spawn captures, await results):

```rust
enum SendableValue {
    Int(i64),
    Word(u64),
    Pid(u64),
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

### One-shot I/O suspensions

- A process suspended on a builtin I/O request is resumed **exactly once**, by the runtime, when the I/O completes
- This applies to runtime I/O suspensions only. Local algebraic-effect continuations (`perform`/`resume`) are a separate mechanism and can currently be resumed more than once; see [`../language/effects.md`](../language/effects.md#current-continuation-semantics)

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

See [error-handling.md](../language/error-handling.md) for the standard library/application error-layering pattern.

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

For GC trigger points, root-set details, latency implications, and the relation
between `run_slice` reductions and non-preemptive GC sections, see
[`execution-bounds.md`](execution-bounds.md).

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

`wait_any` uses deterministic list-order semantics. `wait_any [p1, p2, ..., pn]` blocks until at least one listed child has completed, then returns the leftmost PID in the original input list whose child is complete. If several children complete concurrently, input list order breaks ties; the child that wakes the parent is only a notification, not automatically the winner.

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
- a child that is blocked (on `await`, `wait_any`, a timer or other I/O)
  terminates as `Cancelled` immediately; its pending timer or I/O request is
  cancelled with the I/O backend, and a late result is discarded
- a runnable or running child observes it at the start of its next slice
- cancelling an already-finished child is a no-op
- `Fiber.join` on a cancelled child returns `Result.Err ...`

### Semantics

- `spawn` creates a new isolated process
- `join`/`await_process` block until completion
- result is delivered exactly once

### Process lifecycle

A process is live until it becomes terminal: done, failed, or cancelled.
Both runtimes make a process terminal in a single place, whatever the cause
(normal return, runtime failure, a failed raw await, cancellation, parent
exit, an I/O or spawn failure, or deadlock detection). That transition, in
order:

1. records the outcome: a tombstone for a child whose parent is still live,
   nothing for a child whose parent is already terminal, the final status for
   a root process;
2. cancels every live child of the process (scope exit), recursively, and
   removes the tombstones of its terminated, unconsumed children;
3. wakes the processes blocked in `await` or `wait_any` on it.

So children never outlive their parent: when a parent exits without awaiting
a child, the child is cancelled, and so are its own children.

Tombstones hold a terminated child's outcome until it is consumed. A tombstone
is removed when:

- the parent consumes it with `await_process` or `await_process_result`
  (`Fiber.join`), or
- the parent terminates.

A long-running process that spawns and awaits children in sequence therefore
keeps no per-child state. A child that is terminated but never awaited (for
example cancelled and never joined) keeps its tombstone until its parent
terminates.

After its tombstone is removed a pid is *reaped*. The runtime cannot tell a
reaped child of the caller from a reaped process of another parent, so for
any reaped pid:

- `await_process_result` (`Fiber.join`) returns `Err AlreadyJoined`;
- `await_process` fails the caller with "child result already consumed";
- `cancel` is a no-op;
- `wait_any` treats it as complete.

A pid that belongs to another parent and is still live or unconsumed is
rejected as "not a child"; a pid that was never allocated as "unknown".

On the threaded runtime, a process blocked on a child registers itself, then
parks, then rechecks the child; a terminating child records its outcome
before it looks for waiters. Whichever runs second sees the other, so a wakeup
cannot be lost. If no process is runnable, no I/O is pending, and some process
is still blocked on another process, the runtime fails the blocked processes
with "deadlock: process blocked with no possible waker" through the normal
terminal path. With only parents able to await children this cannot happen;
it is a safety net against bookkeeping bugs.

### Shutdown

`run_to_completion` returns once every process is terminal; workers are joined
before it returns. Dropping a `ThreadPoolIoBackend` cancels every pending
timer and does not wait for a request that is inside a blocking call (an HTTP
request or a file read): such a worker thread is detached and its result
discarded. `ThreadPoolIoBackend::shutdown` joins every thread instead.

### Single-threaded runtime

The single-threaded runtime follows the same rules, with one difference: I/O
builtins run inline and block the thread, so a process is never blocked on
I/O there and a sleeping process cannot be cancelled mid-sleep.

---

## Semantic decisions (v1)

### Await

- Only parent may await child
- Result consumed once; the tombstone is then removed (see Process lifecycle)
- Awaiting a reaped pid: `await_process_result` returns `Err AlreadyJoined`,
  `await_process` fails the caller

### Spawn

- Closures are serialized via captured values
- Non-sendable captures → runtime error

---

## Failure model

| Case                        | Behavior                                                                                 |
| --------------------------- | ---------------------------------------------------------------------------------------- |
| Child process failure       | `await_process` fails the parent (`child process failed: ...`), which propagates up a chain of raw awaits; `await_process_result` / `Fiber.join` returns `Err` |
| Child cancelled             | as a failure, with error `Cancelled`                                                     |
| Parent terminates           | its live children are cancelled and its unconsumed tombstones removed                    |
| I/O failure                 | returned as `Result.Err` by APIs that model it; otherwise fails the process              |
| Runtime invariant violation | process failure                                                                          |

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
