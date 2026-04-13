# Hiko Runtime: Erlang-Style Processes with Local Algebraic Effects

## Overview

Hiko's runtime executes many isolated processes on a fixed pool of worker threads. Each process has its own VM, heap, stack, and effect handlers. Communication between processes is by message passing only. No mutable state is shared between processes.

This design combines:
- **Algebraic effects** for direct-style async (no function coloring)
- **Erlang-style isolation** for safety and simplicity
- **Arc-shared immutable leaves** for efficient message passing

## Comparison with alternatives

| | Hiko | OCaml Eio | Erlang/BEAM |
|---|---|---|---|
| Concurrency unit | Isolated process (own heap) | Fiber (shared heap) | Process (own heap) |
| Communication | Message passing (copy shape, share leaves) | Shared memory | Message passing (deep copy) |
| GC | Per-process, independent | Global, stop-the-world | Per-process, independent |
| Async style | Effects (no coloring) | Effects (no coloring) | Receive loops |
| Continuations | Local to process | Local to fiber | None |
| Large payload passing | Zero-copy via `Arc<str>` | Zero-copy (same heap) | Deep copy |

We borrow from Eio: direct-style async via effects, no function coloring.
We borrow from Erlang: isolated processes, message passing, per-process GC.
We add: `Arc`-shared immutable leaves to avoid Erlang's deep copy cost.

## Architecture

```
┌──────────────────────────────────────────────────┐
│                    Runtime                        │
│                                                   │
│  Scheduler (trait — pluggable)                    │
│  ├── enqueue / dequeue / remove / reductions      │
│  │                                                │
│  Process Table: {Pid → Process}                   │
│  Waiters: {Pid → [Pid]}                           │
│  I/O Backend (trait — pluggable)                  │
│                                                   │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐         │
│  │ Worker 0 │ │ Worker 1 │ │ Worker N │         │
│  │ (thread) │ │ (thread) │ │ (thread) │         │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘         │
│       │             │             │               │
│  ┌────▼─────┐ ┌────▼─────┐ ┌────▼─────┐         │
│  │ Process  │ │ Process  │ │ Process  │         │
│  │ VM+Heap  │ │ VM+Heap  │ │ VM+Heap  │         │
│  │ Mailbox  │ │ Mailbox  │ │ Mailbox  │         │
│  └──────────┘ └──────────┘ └──────────┘         │
│                                                   │
│  I/O Backend (trait: polling / io_uring / mock)   │
└──────────────────────────────────────────────────┘
```

## Process

Each process is a complete isolated VM instance.

```rust
struct Pid(u64);

struct Process {
    pid: Pid,
    vm: VM,
    mailbox: VecDeque<SendableValue>,
    status: ProcessStatus,
    parent: Option<Pid>,
    result: Option<SendableValue>,
}

enum ProcessStatus {
    Runnable,
    Blocked(BlockReason),
    Done,
    Failed(String),
}

enum BlockReason {
    Receive,
    Await(Pid),
    Io(IoToken),
}
```

A process owns:
- Its VM (heap, stack, frames, handlers, globals)
- Its mailbox (incoming messages)
- Its status (runnable, blocked, done)

A process does NOT share:
- Heap objects with other processes
- Stack frames or continuations
- Handler state

## Message passing

### SendableValue

Only `SendableValue` crosses process boundaries. It contains no process-local references.

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

**Rule: copy the shape, share the large immutable leaves.**

- Scalars: copied by value (zero cost)
- Strings/Bytes: `Arc` clone (atomic increment, zero-copy)
- Tuples/Lists/Data: structural copy, leaf sharing

What is NOT sendable: closures, continuations, `Rng` state, `GcRef`.

### Conversion

```rust
fn serialize(value: Value, heap: &Heap) -> Result<SendableValue, String>
fn deserialize(msg: SendableValue, heap: &mut Heap) -> Value
```

### HeapObject representation

To enable zero-copy string sharing, the VM's internal string representation uses `Arc<str>`:

```rust
enum HeapObject {
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    Tuple(Fields),
    Data { tag: u16, fields: Fields },
    Rng { state: u64, inc: u64 },
    Closure { proto_idx: usize, captures: Arc<[Value]> },
    Continuation { saved_frames: Vec<SavedFrame>, saved_stack: Vec<Value> },
}
```

This makes `VM string → SendableValue::String` just `Arc::clone` — one atomic increment.

## Effects

Effects are **entirely process-local**. No changes to existing effect opcodes (`InstallHandler`, `Perform`, `Resume`).

```
Process A                     Scheduler
─────────                     ─────────
perform Yield 42
  handler catches it
  resume k ()
  continues running           (scheduler never involved)
  ...
  fuel runs out               ← Yield to scheduler
                               enqueue A
```

Regular effects (state, generators, error handling) never interact with the scheduler. Only process-level operations (`spawn`, `send`, `receive`, `await`) cause scheduler transitions.

### Async I/O via effects

```sml
(* User writes blocking-looking code *)
val data = perform Read fd

(* Runtime handler: *)
(* 1. Capture continuation k *)
(* 2. Mark process Blocked(Io) *)
(* 3. Register fd with I/O backend *)
(* 4. Yield to scheduler *)
(* 5. When I/O completes: resume k with data *)
```

The effect system is the suspension mechanism. The I/O backend is the completion mechanism. The user code looks synchronous.

## Scheduler

The scheduler is behind a trait so it can be replaced without touching the rest of the runtime.

### Trait

```rust
/// Scheduling decisions are isolated behind this trait.
/// The runtime calls into the scheduler; the scheduler never
/// reaches into runtime internals.
trait Scheduler: Send + Sync {
    /// A process became runnable (new, yielded, or unblocked).
    fn enqueue(&self, pid: Pid);

    /// Block until a runnable process is available, then return it.
    /// Called by each worker thread.
    fn dequeue(&self) -> Pid;

    /// A process finished or failed — remove it from scheduling.
    fn remove(&self, pid: Pid);

    /// Hint: how many reductions to grant this process.
    /// The scheduler can vary this per process for fairness tuning.
    fn reductions(&self, pid: Pid) -> u64;
}
```

### Bundled implementations

**`FifoScheduler`** (default) — simple FIFO queue. Fixed reduction count. Good enough for most workloads.

```rust
struct FifoScheduler {
    queue: Mutex<VecDeque<Pid>>,
    notify: Condvar,
    reductions: u64,
}
```

Future implementations (not in v1):
- **`PriorityScheduler`** — per-process priority levels, higher priority dequeued first
- **`WorkStealingScheduler`** — per-worker queues with stealing for load balance
- **`FairScheduler`** — tracks accumulated reductions, reduces slice for long-running processes

### Worker loop

Workers interact with the scheduler only through the trait:

```
loop {
    pid = scheduler.dequeue()          // blocks if empty
    reductions = scheduler.reductions(pid)
    process = processes.take(pid)

    match process.vm.run_slice(reductions) {
        Yield       → scheduler.enqueue(pid)
        Done(value) → scheduler.remove(pid); wake awaiters
        Spawn(fn)   → create child; scheduler.enqueue(child_pid)
        Send(pid,v) → push to mailbox; scheduler.enqueue(target) if blocked
        Receive     → pop mailbox or mark blocked
        Await(pid)  → check done or mark blocked
        Io(req)     → register with backend; mark blocked
    }
}
```

The worker loop never makes scheduling decisions — it reports events, the scheduler decides ordering.

### Preemption

Each process gets N opcode executions per slice (from `scheduler.reductions(pid)`). After N reductions, the VM yields. No process can starve others. The scheduler controls N, allowing different policies without changing the worker loop.

## Garbage collection

### Per-process, independent

Each process has its own heap and its own GC. When one process needs collection, only that process pauses. All other processes continue running.

### Root set

For one process:
- Operand stack
- Call frames (locals, captures)
- Handler frames
- Captured continuations
- Global variables

NOT roots: other processes, scheduler state, mailbox contents.

### Mailbox boundary

- Mailbox stores `SendableValue` (outside the process heap)
- Receiving deserializes into the local heap
- GC never traces mailbox contents

### Arc leaves and GC

- `Arc<str>` inside `HeapObject` is an opaque Rust payload
- Collecting a heap object drops the `Arc`, decrementing the refcount
- The data lives until the last `Arc` is dropped (possibly in another process)
- No special GC handling needed

### Key invariant

**No pointer from process A's heap into process B's heap.**

With `SendableValue` as the boundary, this holds naturally.

## User-facing API

```sml
(* Spawn a new process *)
val child = spawn (fn () => compute_something ())

(* Wait for a process to complete *)
val result = await child

(* Send a message *)
val _ = send (child, JStr "hello")

(* Receive a message (blocks until available) *)
val msg = receive ()

(* Run two things concurrently *)
fun both f g =
  let val t1 = spawn f
      val t2 = spawn g
  in (await t1, await t2) end
```

## I/O backend

Abstract trait — implementation chosen at runtime startup:

```rust
trait IoBackend: Send + Sync {
    fn register(&self, token: IoToken, interest: IoInterest);
    fn poll(&self, timeout: Duration) -> Vec<(IoToken, IoResult)>;
}
```

Supports readiness-based (epoll, kqueue) and completion-based (io_uring) backends. The VM never directly touches the I/O backend.

## Safety

### What Rust enforces

| Invariant | Mechanism |
|---|---|
| `SendableValue` has no `GcRef` | Enum definition |
| `SendableValue` is `Send + Sync` | Rust type system |
| Continuations not sendable | Not a variant |
| `Arc<str>` is thread-safe | `Arc` is `Send + Sync` |

### What can go wrong (not memory corruption)

- Memory retention: process holds `Arc<str>` to large string after done with it
- Deadlock: process A awaits B, B awaits A
- Starvation: process runs expensive pure computation (mitigated by reduction counting)

### What cannot happen in safe Rust

- Data races on shared mutable state (no shared mutable state)
- Use-after-free across processes (no cross-process pointers)
- Concurrent GC bugs (no concurrent GC)

## Implementation milestones

| Milestone | Scope | Threading |
|---|---|---|
| 1 | `SendableValue`, `Process`, single-threaded scheduler, `spawn`/`await` | 1 thread |
| 2 | Thread pool, `crossbeam` channels, work distribution | N threads |
| 3 | `send`/`receive` mailbox messaging | N threads |
| 4 | I/O backend trait, `polling`-based implementation | N+1 threads |
| 5 | `Rc` → `Arc` in VM for work-stealing | N threads |
| 6 | `HeapObject::String(String)` → `HeapObject::String(Arc<str>)` | N threads |

## Design principles

1. **Prefer isolation over sharing** — no shared mutable state, ever
2. **Prefer copying over synchronization** — copy the shape, share immutable leaves
3. **Prefer simplicity over elegance** — explicit state machines, boring Rust code
4. **Effects are control flow, not I/O** — the runtime interprets effects as I/O
5. **GC is local** — never stop the world, always stop one process
6. **The VM doesn't change** — processes are just multiple VM instances
7. **Pluggable policies** — scheduler and I/O backend are traits, swappable without touching the runtime core
