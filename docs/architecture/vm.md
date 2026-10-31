# VM Structure and Process Creation

## Overview

The VM is split by responsibility:

- `crates/hiko-vm/src/vm/mod.rs`: `VM` state, constructors, public surface
- `crates/hiko-vm/src/vm/runtime_bridge.rs`: `RunResult`,
  `RuntimeRequest`, slice transitions, child VM creation
- `crates/hiko-vm/src/vm/dispatch.rs`: opcode interpreter, stack helpers, operand decoding
- `crates/hiko-vm/src/vm/builtins.rs`: builtin registration and builtin dispatch
- `crates/hiko-vm/src/vm/gc.rs`: allocation helpers and root-set calculation
- `crates/hiko-vm/src/vm/host.rs`: output sinks, stdin injection, `exec` preparation/execution

This keeps the runtime-facing contract narrow while keeping execution-local
state inside `VM`.

Operational assumption: Hiko VMs are typically short-lived and run for seconds
to minutes. That makes correctness, isolation, and clear teardown the primary
optimization targets; long-horizon concerns such as heap compaction,
cache-pruning, or daemon-runtime tuning are secondary unless the embedding
model changes.

## State Ownership

Execution-local state is owned by `VM`:

- cancellation (`VM::request_cancellation`)
- slice fuel, and the process tree's shared fuel budget
- stack, call frames, handler frames
- per-process heap and string cache

`Process` owns lifecycle metadata that the runtimes need:

- `pid`
- `status`
- `parent`
- `children`: children whose result the process has not consumed yet
- scope membership

The runtime should treat `RunResult` as the only public execution transition.
It can request cancellation or resume blocked work, but it should not mutate VM
internals directly.

## Execution Boundaries

For the full embedder-facing contract around `VM::run`, `VM::run_slice`, GC,
and non-preemptive sections, see [`execution-bounds.md`](execution-bounds.md).

`VM::run()` is an unbounded convenience entry point. It runs until the
program halts, fails, exhausts configured fuel, or emits a runtime request. If
no fuel limit is configured, `run()` is non-preemptive: the dispatch loop has no
reduction boundary and a non-terminating program can keep control indefinitely.

Embedders that need bounded execution, scheduler fairness, or regular
cancellation observation should use `VM::run_slice(reductions)`. The slice API
installs a per-call reduction budget and returns a `RunResult` boundary that the
runtime can map back into process state. Slice reductions bound opcode dispatch;
they do not preempt native builtins, synchronous host calls, or GC in the middle
of their Rust implementations.

## Runtime/VM Transition Contract

The steady-state flow is:

1. A runtime dequeues a runnable process.
2. It temporarily owns the `Process` and calls `VM::run_slice(reductions)`.
3. `run_slice` interprets bytecode until one of four boundaries:
   - normal completion
   - failure
   - reduction budget exhaustion
   - a runtime request emitted by a builtin
4. The runtime maps `RunResult` back into process-table state:
   - `Done` / `Failed` / `Cancelled`: terminal
   - `Yielded`: runnable again
   - `Spawn`: create child VM/process, then resume parent
   - `Await` / `AwaitResult` / `WaitAny`: blocked on child state
   - `Io`: blocked on the I/O backend

The threaded runtime has one extra state transition: while a process is running,
it is removed from `processes` and exists only on the worker thread stack.
`live`, `waiters`, `any_waiters`, `io_waiters`, `pending_cancels`, and
`tombstones` are the runtime-side structures that preserve visibility across
that gap. Any party that changes a process's state first takes it out of
`processes`; see [`runtime.md`](runtime.md#process-lifecycle) for the
terminal transition and the wake protocol.

## Process Creation Path

Creating a child process currently means:

1. `runtime_ops::create_child_vm_from_parent`
2. `VM::create_child`
3. deserialize spawn captures into the child heap
4. `VM::setup_closure_call`
5. wrap in `Process`
6. insert into the runtime-specific process table

The expensive part is step 2 plus capture deserialization. The runtime table
insert is intentionally kept out of the VM layer.

### Current cost model

`VM::create_child`:

- reuses compiled bytecode/functions/effects through `Arc`
- shares the parent's builtin table (`Arc<BuiltinTable>`); the child's global
  table starts empty and caches a builtin or inherited global on first use
- shares the parent's filesystem policy and its opened directory capabilities
  (`Arc<FsPolicy>`), so no directory is reopened at spawn
- clones the remaining capability configuration (exec/http)
- reuses already-resolved `exec` allowlist paths instead of re-walking `PATH`

`create_child_vm_from_parent`:

- allocates the temporary `Vec<Value>` used for deserialized captures
- allocates child heap objects for any captured strings/tuples/data
- installs one initial `CallFrame`
- allocates one `Arc<[Value]>` for the closure capture slice

That means the cost is dominated by:

- captured value deserialization
- the child's fixed buffers (stack, frames, heap)

It is not dominated by compiled-program cloning.

## Measuring Cost

Use the example benchmark:

```bash
cargo run -p hiko-vm --example process_creation_cost --release
```

It prints:

- wall-clock time per operation
- allocator call counts per operation
- approximate allocated bytes per operation

The example measures:

- `VM::create_child`
- `VM::create_child` from a parent with a filesystem policy
- full spawn path with zero captures
- full spawn path with a small capture set

[`performance.md`](performance.md) has the full benchmark set and the
before/after numbers for PR #104.

These numbers are machine- and allocator-dependent, so the example is the source
of truth. Keep this example updated when the process creation path changes.

### Current sample output

Sample output from `cargo run -p hiko-vm --example process_creation_cost --release`
on October 31, 2026, Apple M1 Max:

```text
Measuring hiko process creation costs over 20000 iterations.
This excludes scheduler/table insertion and focuses on VM creation.

VM::create_child                    334 ns/op    7.00 alloc    0.00 realloc    7.00 free    1856.0 B/op
VM::create_child (fs policy)        333 ns/op    7.00 alloc    0.00 realloc    7.00 free    1856.0 B/op
spawn path (0 captures)             389 ns/op    8.00 alloc    0.00 realloc    8.00 free    1872.0 B/op
spawn path (4 captures)            1056 ns/op   33.00 alloc    0.00 realloc   33.00 free    3552.0 B/op
```

Use those numbers as a baseline, not as a stable contract.
