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
| Effect handlers       | Local to process               | Local to fiber         | None                     |
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

- VM state (heap, stack, frames, effect handlers)
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
    Closure { proto_idx: usize, captures: Vec<SendableValue> },
    Builtin(u16),
}
```

Rule:

> **Copy the shape, share immutable leaves**

A closure is sent as its function index and its captures. Every process runs
the same compiled program and registers the same builtins in the same order,
so the index means the same function in the child. `Rng`
values and host handles cannot be sent; trying to
send one is a runtime error that names the kind of value.

Copying is iterative and bounded by `max_message_bytes` and a depth limit. See
[Budgets across process boundaries](execution-bounds.md#budgets-across-process-boundaries).

### What a child process can see

A child runs one closure and never runs the main chunk. It can use:

- its captures, copied as above;
- every builtin;
- every top-level binding (functions, constructors, structure members and
  values) that the parent had defined when it called `spawn`.

At `spawn`, the parent gives the child a snapshot of its top-level bindings
as `SendableValue`s. The child deserializes a binding the first time it reads
it, so it pays only for what it uses. The parent serializes each binding once
and shares the snapshot between children (and their children) until it
defines another top-level binding. Bindings are immutable, so a child sees the
same values the parent sees.

A binding that cannot be sent is recorded with the reason. A child that reads
it fails with `top-level `name` cannot be used in a child process: <reason>`.
A child that never reads it is not affected. This is a runtime check, not a
compile-time one.

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

`exec` follows the same split. The VM resolves and authorizes the executable
and arguments before suspending. A threaded runtime submits the owned,
pinned command to its I/O backend; it never runs the command on a VM worker.
The exec deadline includes time queued in the backend. Cancellation before
start prevents spawning; cancellation during execution kills the command's
process group, reaps its direct child and joins its output readers before
returning. Timeout, output overflow and backend shutdown use the same cleanup.
Late results cannot resume a cancelled process. Output limits, executable
identity checks, argument rules and cumulative I/O accounting remain enforced.

The synchronous `VM::run` and `Runtime` embedding paths use the same command
runner inline. They block the calling thread and do not promise cancellation
or timer progress while an exec is running.

### Testing

Use a mock I/O backend (`MockIoBackend`) for deterministic testing. The backend is pluggable at the runtime level, not at the language level.

---

## I/O semantics

### One-shot I/O suspensions

- A process suspended on a builtin I/O request is resumed **exactly once**, by the runtime, when the I/O completes
- This applies to runtime I/O suspensions only. Local algebraic effects (`perform`/`resume`) are a separate mechanism without first-class continuations; see [`../language/effects.md`](../language/effects.md)

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
- handlers, including handlers hidden while a provider clause runs

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

The threaded I/O backend publishes a queued HTTP request and its deadline under
the same timer lock, taking locks in timer then work-queue order. Cancellation
and expiry see a complete registration. Cancellation keeps that lock until both
the timer and queued job are removed. A worker releases the queue lock before
cancelling its queue timer. Refused publication rolls the deadline back; repeated
cleanup remains harmless. The runtime also rechecks cancellation after backend
registration, covering cancellation before the backend knew the request token.

### Semantics

- `spawn` creates a new isolated process
- `join`/`await_process` block until completion
- result is delivered exactly once

### Owned bulk joins

`Fiber.all` and `Fiber.join_all` use one owned runtime group, rather than
repeated `wait_any` scans and individual joins. Results retain input order;
`Fiber.both` retains its two result types through `Either` wrappers. An empty
group succeeds, subject to the ordinary aggregate-envelope, host-work and heap
limits. The sequential runtime has the same outcomes, but inline I/O still
provides no threaded fail-fast latency guarantee.

Admission checks the entire bounded input before observing ready failures.
Duplicates and reaped pids produce `AlreadyJoined`; that means **result
unavailable/reaped; ownership no longer known**, not proof of a previous join.
Known live or unconsumed foreign processes produce a typed "not a child" error;
unknown pids produce a typed unknown-pid error. Input-order admission invalidity
precedes child failures. After valid admission, the leftmost initially ready
failure wins; thereafter the first observed failure wins.

A group-local typed error cancels and reaps only distinct, positively owned
children named in the bounded input. Foreign outcomes are never consumed.
Cleanup preserves its selected cause and uses reserved slots and wakeups even
when ordinary admission/host-work quota is spent. The parent resumes only after
accepted members terminate and their exec/backend-reader cleanup completes.
This does **not** change parent scope exit: cancellation or failure of the
parent still cancels **all** its children, including children outside the group.

The independent `limits.max_join_group_bytes` policy defaults to 64 MiB and is
shared across the process tree. Freeze it before execution; changing policy
must not reset live usage. Groups reserve logical weights of 512 bytes per
group plus 256 bytes per input entry and accept at most 10,000 entries,
including duplicates and reaped pids. These bounds account for group metadata
and retained serialized results/errors, not exact RSS or all runtime tombstones.
Each adopted completion spends its checked serialized footprint. Incremental
aggregate list/protected-result envelope accounting also enforces the parent's
`max_message_bytes`, independently of each child's message limit. Credits stay
held through transfer and are refunded exactly once when ownership is released.
Quota, host-work and transfer-allocation failures fail the parent after cleanup;
they are not recoverable typed quota errors. Partial result construction remains
rooted and is never published as a successful partial list.

`capabilities.fiber.fiber_await_all` is a default-off bulk capability under
`builtin-process`. It grants await/cancel/reap authority only over accepted
children; spawn requires its own grant. Individual await/cancel/wait-any leaves
may remain denied. Protected process wrappers resolve each leaf lazily, so a
non-strict bulk-only import does not request denied peers. Strict compilation
continues to check references in unused bodies; no permission is auto-granted.

Runtime bookkeeping uses a pid-to-input-index table, one initial ready pass and
one final ordered pass: expected O(n) member work plus O(B) transferred payload
work, not a full member scan for each completion. Register/park/recheck and group
identity prevent lost wakes, double moves and double resumption; callbacks never
run while registration locks are held. Parent teardown drains registrations and
releases group ownership.

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

For example, `val q = await_process (spawn (fn () => spawn (fn () => 41)))`
returns a grandchild's pid after its parent exits. Once q is reaped,
`await_process q` fails with "child result already consumed", and
`await_process_result q` returns
`Err AlreadyJoined`. Neither operation successfully joins that grandchild.
The diagnostic cannot distinguish this case from an actual second join;
runtime process errors also lack source spans. During threaded parent cleanup,
the grandchild may still be classified as foreign and rejected as "not a child".
Keep the spawning parent live and await its child there.

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
timer and kills/reaps running exec jobs, joining their output readers. It does
not wait for other requests inside a blocking call (an HTTP
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
