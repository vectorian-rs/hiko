# VM Structure and Process Creation

## Overview

The VM is split by responsibility:

- `crates/hiko-vm/src/vm/mod.rs`: `VM` state, constructors, public surface
- `crates/hiko-vm/src/vm/runtime_bridge.rs`: `RunResult`, `RuntimeRequest`, slice transitions, child VM creation
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
- blocked continuation root (`blocked_continuation`)
- slice fuel / persistent remaining fuel
- stack, call frames, handler frames
- per-process heap and string cache

`Process` owns lifecycle metadata that the runtimes need:

- `pid`
- `status`
- `parent`
- scope membership

The runtime should treat `RunResult` as the only public execution transition.
It can request cancellation or resume blocked work, but it should not mutate VM
internals directly.

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
`child_parents`, `waiters`, `any_waiters`, and `tombstones` are the runtime-side
structures that preserve visibility across that gap.

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
- rebuilds builtin/global tables for a fresh interpreter instance
- clones immutable capability configuration (exec/fs/http)
- pre-reserves builtin/global table capacity to reduce allocator churn
- reuses already-resolved `exec` allowlist paths instead of re-walking `PATH`

`create_child_vm_from_parent`:

- allocates the temporary `Vec<Value>` used for deserialized captures
- allocates child heap objects for any captured strings/tuples/data
- installs one initial `CallFrame`
- allocates one `Arc<[Value]>` for the closure capture slice

That means the cost is dominated by:

- builtin/global table rebuild
- capability cloning
- captured value deserialization

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
- full spawn path with zero captures
- full spawn path with a small capture set

These numbers are machine- and allocator-dependent, so the example is the source
of truth. Keep this example updated when the process creation path changes.

### Current sample output

Sample output from `cargo run -p hiko-vm --example process_creation_cost --release`
run on April 21, 2026 in the development environment used for this refactor:

```text
Measuring hiko process creation costs over 20000 iterations.
This excludes scheduler/table insertion and focuses on VM creation.

VM::create_child                   7913 ns/op  104.00 alloc    0.00 realloc  104.00 free    9836.0 B/op
spawn path (0 captures)            7897 ns/op  106.00 alloc    0.00 realloc  106.00 free   10012.0 B/op
spawn path (4 captures)            8058 ns/op  110.00 alloc    0.00 realloc  110.00 free   10668.0 B/op
```

Use those numbers as a baseline, not as a stable contract.
