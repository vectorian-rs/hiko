# Execution Bounds, GC, and Non-Preemptive Sections

This note is the embedder-facing contract for bounded execution latency in the
current Hiko VM/runtime. For structural ownership, see [`vm.md`](vm.md) and
[`runtime.md`](runtime.md).

## Summary

Hiko execution is **cooperative**, not preemptive.

- `VM::run()` is a convenience API that runs until completion, failure, fuel
  exhaustion, or a runtime request.
- `VM::run_slice(reductions)` is the scheduler API. It installs a temporary
  opcode/reduction budget and returns a `RunResult` boundary.
- Cancellation is observed at slice boundaries, before a slice starts.
- GC is per-process/per-VM. A collection pauses only the process that triggered
  it; there is no global stop-the-world collector.
- Native builtins, synchronous host calls, deserialization, and GC collections
  are non-preemptive once entered. They are bounded by explicit resource limits
  where configured, but they do not yield midway through their internal Rust
  work.

## `VM::run()` vs `VM::run_slice()`

### `VM::run()`

Use `VM::run()` for short one-shot scripts where the caller is willing to let
the VM run until a terminal state or runtime request.

`run()` stops when:

- the program completes;
- execution fails;
- configured persistent fuel/work is exhausted;
- a builtin emits a runtime request such as spawn/await/I/O.

If no fuel/work limit is configured, `run()` has no reduction boundary. A
non-terminating pure bytecode loop can retain control indefinitely.

### `VM::run_slice(reductions)`

Use `run_slice` for embedders that need fairness, cancellation observation, or
latency control.

A slice does the following:

1. validates startup state;
2. caps the requested reduction budget by remaining persistent `max_work`;
3. observes any previously requested cancellation;
4. ensures the root frame exists;
5. dispatches bytecode until completion, failure, fuel exhaustion, or a runtime
   request;
6. updates remaining persistent fuel/work;
7. returns a `RunResult` for the runtime to map back into process state.

A reduction is currently one VM opcode dispatch. Host work inside one opcode or
builtin is not subdivided into reductions.

## Runtime boundaries

The runtime sees these boundaries via `RunResult`:

| Boundary | Runtime action |
| --- | --- |
| `Done` | mark terminal; deliver result to parent/awaiters |
| `Failed` | mark terminal; wake awaiters with failure |
| `Cancelled` | mark terminal as cancelled |
| `Yielded` | re-enqueue the process |
| `Spawn` | create a child process, then resume parent |
| `Await` / `AwaitResult` / `WaitAny` | block until child state changes |
| `Io` | register request with the I/O backend and block |

The single-threaded and threaded runtimes both use this contract. The threaded
runtime additionally removes a running process from the process table while a
worker owns it; runtime waiters/tombstones preserve visibility for joins and
`wait_any` during that interval.

## Non-preemptive sections

The following operations do not yield in the middle of their Rust implementation:

| Section | Limit/accounting | Notes |
| --- | --- | --- |
| One opcode dispatch | `max_work` / slice reductions | Dispatch checks fuel between opcodes, not inside an opcode. |
| Native sync builtins | `max_host_work`, `max_io_bytes`, `max_memory_bytes` where applicable | JSON, regex, random, string, bytes, hash, HTTP/file, and process-boundary paths have first-pass accounting. |
| Synchronous HTTP/file/exec reads | `max_io_bytes` | Bounded readers reject oversized results before allocating/returning them. |
| Async I/O worker execution | request carries remaining `max_io_bytes`; completion is charged on delivery | The process is suspended while the worker runs. The worker itself is not preempted by VM reductions. |
| Process-boundary serialization/deserialization | `max_host_work`, heap/memory limits | Used for spawn captures and child results. |
| GC collection | heap/memory thresholds and limits | Mark/sweep is local to one VM and runs to completion. |
| Output formatting/display | `max_io_bytes` for stdout bytes; heap/memory for generated strings | Large display work is not preempted mid-format. |

This is the main latency rule: **slice reductions bound bytecode dispatch, not
arbitrary host code duration**. Host-code duration is bounded by explicit byte,
heap, and host-work limits plus the runtime cost of the underlying operation.

## Garbage collection behavior

Each VM owns one heap. Each process owns one VM. Therefore GC is process-local:

- one process triggering GC does not scan or stop other process heaps;
- there are no cross-process heap pointers;
- values crossing a process boundary are converted through `SendableValue`,
  which copies shape and shares immutable leaves where possible.

The collector is mark-and-sweep with free-list reuse. It is non-compacting: a
`GcRef` remains an arena index while the object is live.

### GC trigger points

GC may run:

- before allocating a heap object if the adaptive threshold says collection is
  due;
- before allocating a heap object if the allocation would exceed heap/object or
  byte limits, giving unreachable objects a chance to be reclaimed before a
  controlled limit error;
- at slice/runtime-request boundaries when boundary collection is due.

### Root set

The VM root set includes:

- value stack;
- call-frame captures;
- globals;
- handler captures;
- string cache entries;
- blocked continuation roots;
- request-local extra roots for objects being allocated.

The runtime does not compute GC roots. Root discovery stays inside the VM layer.

### GC latency implication

A GC collection runs to completion once started. `run_slice` cannot stop in the
middle of marking or sweeping. The pause is local to that process, but the worker
thread running that process is occupied for the duration of the collection.

Heap limits reduce worst-case GC work by limiting live heap size. Boundary GC
reduces accumulation of request-local garbage across slices, but it is still a
cooperative collection point, not a preemptive collector.

## Resource limits and what they bound

| Limit | Bounds | Does not directly bound |
| --- | --- | --- |
| `max_work` / `max_fuel` | VM opcode dispatches across execution | Native builtin CPU time inside one opcode |
| `run_slice(reductions)` | one scheduling slice's opcode dispatch budget | Native builtin, GC, or sync host-call duration once entered |
| `max_host_work` | approximate CPU-bound builtin/host work units | Wall-clock time exactly |
| `max_io_bytes` | stdin/stdout, HTTP/file/exec payload bytes, async completion bytes | Number of requests unless separate capability policy denies them |
| `max_memory_bytes` | accounted heap bytes | Native resources held behind host handles |
| `max_host_resources` / `limits.host_resources` | total and per-kind live opaque native host handles | Size of the native object behind each handle |
| `max_heap` | live heap object count | Object byte size |
| Stack/frame constants | value stack and call-frame depth | Heap or host resource size |

## Embedder guidance

For latency-sensitive use:

1. Prefer `run_slice` over `run`.
2. Set both a persistent `max_work` and a per-slice `reductions` budget.
3. Set `max_host_work` for JSON/regex/hash/string/bytes/random-heavy scripts.
4. Set `max_io_bytes` for scripts that can read, write, fetch, print, or exec.
5. Set `max_memory_bytes` and, where useful, `max_heap`.
6. Set `max_host_resources` and per-kind `limits.host_resources` caps when scripts can create native handles such as AWS clients.
7. Use the runtime-managed async I/O path for operations that may block.
8. Treat sync HTTP/file/exec builtins as bounded but non-preemptive host calls.
9. Size process heaps so local GC pauses are acceptable for your application.

A practical starting point for agent/tool workloads is:

```toml
[limits]
max_work = 10_000_000
max_host_work = 10_000_000
max_io_bytes = 67_108_864
max_memory_bytes = 268_435_456
max_host_resources = 32
max_heap = 500_000

[limits.host_resources]
aws_config = 8
aws_s3_client = 8
aws_sqs_client = 8
```

Then tune using representative scripts. Increase `max_work` for pure Hiko loops,
`max_host_work` for CPU-heavy builtins, `max_io_bytes` for large payloads,
memory/heap limits for large live data structures, and host-resource limits for
scripts that intentionally retain many native handles.

## Current non-goals

Hiko does not currently provide:

- OS-level preemption of a running VM thread;
- preemption inside a Rust builtin;
- preemption inside GC marking/sweeping;
- exact wall-clock accounting for host work;
- per-operation byte knobs such as `max_http_response_bytes` separate from the
  global `max_io_bytes`.

These are explicit boundaries of the current runtime contract, not accidental
omissions.
