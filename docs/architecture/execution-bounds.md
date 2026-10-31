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
2. observes any previously requested cancellation;
3. reserves the requested reductions from the process tree's `max_work`
   budget, or whatever is left of it (see
   [Budgets across process boundaries](#budgets-across-process-boundaries));
4. ensures the root frame exists;
5. dispatches bytecode until completion, failure, fuel exhaustion, or a runtime
   request;
6. returns the reductions it did not use to the tree's budget;
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
| Synchronous HTTP/file/exec reads | per-operation byte limits and `max_io_bytes`; HTTP and exec timeouts | Bounded readers stop one byte past the limit, so an oversized result is never buffered in full. See [Per-operation I/O limits](#per-operation-io-limits). |
| Async I/O worker execution | request carries the per-operation byte limit and HTTP timeouts; completion is charged on delivery | The process is suspended while the worker runs. The worker itself is not preempted by VM reductions. |
| Process-boundary serialization/deserialization | `max_message_bytes`, `max_host_work`, heap/memory limits | Used for spawn captures and child results. Iterative, with a depth limit; see [Budgets across process boundaries](#budgets-across-process-boundaries). |
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
- handler captures, including handlers hidden while a provider clause runs;
- string cache entries;
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
| `max_http_response_bytes`, `max_file_read_bytes`, `max_exec_output_bytes` | bytes read by one HTTP response, file read, or exec stream | Cumulative bytes across calls (that is `max_io_bytes`) |
| `http_timeout_ms`, `http_connect_timeout_ms` | wall-clock duration of one HTTP call, including the body | Total time spent across many calls |
| `exec_timeout_ms` | wall-clock duration of one `exec` call | Processes that detach from the child's process group |
| `max_memory_bytes` | accounted heap bytes | Native resources held behind host handles |
| `max_host_resources` / `limits.host_resources` | total and per-kind live opaque native host handles | Size of the native object behind each handle |
| `max_heap` | live heap object count of one process | Object byte size |
| `max_live_processes`, `max_spawns` | spawned processes alive at once, and spawned in total, per process tree | Work each process does (that is `max_work` and the other tree budgets) |
| `max_message_bytes` | size of one serialized spawn capture or child result | How often values are sent |
| Stack/frame constants | value stack and call-frame depth | Heap or host resource size |

## Budgets across process boundaries

A root process and every process it spawns, directly or indirectly, form a
process tree. Fuel (`max_work`), `max_memory_bytes`, `max_io_bytes` and
`max_host_work` are budgets for the whole tree: each is one shared counter
that every process in the tree charges, so spawning children cannot
multiply a script's budget.

- **Fuel.** Each scheduling slice reserves its reductions from the tree's
  budget before it runs and returns what it did not use afterwards, so
  processes running at the same time on different threads can never spend
  more than the budget between them. Near the end of the budget, a process
  can find the budget empty while another process still holds an unused
  reservation for its current slice.
- **I/O bytes and host work** are charged to the tree as they are used.
- **Memory** is the sum of the live heaps in the tree. A heap returns bytes to
  the tree's budget when garbage collection frees them and when its process
  ends. Each process collects only its own heap, so a process can fail with a
  memory error because of data that another live process in the tree holds.
- `max_heap` (an object count) and the per-operation limits stay per process;
  children inherit the parent's values.

One consequence: a child that runs out of fuel has used up the tree's fuel,
so its parent cannot recover and fails too when it next runs. A child that
fails for other reasons, or finishes, leaves the rest of the budget to the
tree.

Spawning is bounded by two more limits, counted per tree without the root:

| Key | Default | Bounds |
| --- | --- | --- |
| `max_live_processes` | 10,000 | spawned processes alive at once; a process frees its slot when it ends |
| `max_spawns` | unlimited | processes spawned over the tree's lifetime |

A spawn over either limit fails the spawning process.

Values that cross a process boundary (spawn captures and child results) are
copied by `sendable.rs` with explicit work stacks rather than recursion, so
a deeply nested value cannot overflow the Rust stack:

- A value nested deeper than 100,000 levels is rejected. A list counts as one
  level, however long it is.
- `max_message_bytes` (default 64 MiB) bounds the size of one copied value:
  the in-memory size of every node plus the bytes of strings and byte
  arrays. The size is computed before anything is copied, with one memo
  entry per distinct heap object, so a small value with heavy sharing (such as
  `x30` in `val x1 = (x0, x0) ... val x30 = (x29, x29)`, 31 objects that
  expand into 2^31 nodes) fails in time proportional to its objects.
- Strings and byte arrays are shared through `Arc` while in transit, not
  copied.

Exceeding a limit is an error at the spawn or at the child's return, not a
crash.

## Per-operation I/O limits

These `[limits]` keys bound a single builtin call. A policy that omits a key
gets the default.

| Key | Default | Applies to |
| --- | --- | --- |
| `http_timeout_ms` | 30000 | Whole HTTP call, including reading the body: `http_get`, `http`, `http_json`, `http_msgpack`, `http_bytes` |
| `http_connect_timeout_ms` | 10000 | Establishing the HTTP connection |
| `exec_timeout_ms` | 30000 | Wall clock of one `exec` call |
| `max_http_response_bytes` | 67108864 (64 MiB) | One HTTP response body |
| `max_file_read_bytes` | 67108864 (64 MiB) | One `read_file`, `read_file_bytes`, `read_file_tagged`, or `edit_file_tagged` read |
| `max_exec_output_bytes` | 16777216 (16 MiB) | Each of `exec`'s stdout and stderr |
| `max_concurrent_io` | 32 | Async I/O requests running at once per process tree (threaded runtime) |

Rules:

- Each byte limit is combined with the remaining `max_io_bytes` budget; the
  smaller one wins, and the error names the limit that was hit, for example
  `read_file: result exceeds max_file_read_bytes (1024 bytes)`.
- Reads stream and stop one byte past the limit. A response, file, or command
  that produces more is never buffered in full, and nothing is charged to
  `max_io_bytes` for a rejected read.
- Both runtimes apply the same limits. In the threaded runtime, the HTTP
  request runs on an I/O pool thread while the process is suspended, so other
  processes keep running until the request completes or times out. See
  [Async I/O pool](#async-io-pool) for how requests share the pool.
- `exec` runs the child in its own process group. On timeout, or when an output
  stream exceeds `max_exec_output_bytes`, the whole group is killed, including
  grandchildren, and the call fails without waiting for the timeout.
- Exceeding any of these limits is a controlled runtime error, not an `Err`
  value. The raw builtins return plain tuples, so there is no `Result` to put
  the error in. In the threaded runtime, `Std.Fiber.join` reports a child's
  failure as `Result.Err`, which is how a script recovers from one.
- Spawned child processes inherit the parent's per-operation limits.

## Async I/O pool

In the threaded runtime, a process that makes an I/O request is suspended
while the request runs on the I/O backend (`ThreadPoolIoBackend`):

- **Sleeps use no worker.** `sleep` is completed by a timer thread at its
  deadline, however busy the workers are.
- **One shared queue, fair across process trees.** A process tree is a root
  process plus every process it spawns, directly or indirectly. Blocking
  requests (HTTP, file reads, AWS calls) wait in a queue per tree, and a free
  worker takes the next request round-robin across trees.
- **Slow requests do not block others.** Blocking calls hold a worker each, so
  the pool grows when requests are waiting and no worker is idle, up to 256
  threads (`DEFAULT_MAX_IO_THREADS`). Workers above the core count exit after
  five idle seconds. A slow request occupies only its own worker.
- **`max_concurrent_io` caps one tree.** At most that many of a tree's
  requests run at once. Further requests stay queued, and the processes that
  made them stay suspended: one script cannot take over the pool, and other
  trees' requests still start promptly.
- **HTTP deadlines count from the request.** `http_timeout_ms` runs from the
  moment the script makes the request, including time spent queued. A request
  still queued at its deadline fails without starting, with
  `http: timeout: deadline passed while the request was queued`. A request
  that starts late gets only the time left, and one with less than 10 ms left
  fails instead of starting.
- **Cancelled requests are dropped.** Cancelling a process removes its queued
  request; a request already running finishes, and its result is discarded.

The single-threaded runtime runs I/O inline, one request at a time, so these
rules do not apply there.

Module fetching (`import` of remote modules and `hiko lock verify`) does not
follow redirects, and accepts plain `http://` only for loopback hosts
(`localhost`, `127.0.0.0/8`, `::1`). Other hosts must use `https://`.

## Type checking

Type inference counts every type node it visits (substituting, instantiating,
running the occurs check) against `max_type_work`, 10,000,000 per compilation
by default (`hiko_types::infer::DEFAULT_MAX_TYPE_WORK`). Going over the limit
fails the compilation with "type checking exceeded its work limit".

The limit exists because types can grow exponentially in the size of the
source. Each line here doubles the type:

```sml
val x0 = 0
val x1 = (x0, x0)
val x2 = (x1, x1)
(* ... *)
```

and polymorphic functions that apply each other twice do the same. Without
the limit, 30 such lines stall the compiler for many minutes. With it, the
worst inputs found fail in under 1.5 s (release build). Ordinary programs use
far less: the largest program in this repository uses about 30,000, and a
generated 15,000-line program about 1,000,000.

The limit is not a `hiko.toml` key. Code that drives `InferCtx` directly can
change it with `InferCtx::set_max_type_work`.

## Embedder guidance

For latency-sensitive use:

1. Prefer `run_slice` over `run`.
2. Set both a persistent `max_work` and a per-slice `reductions` budget.
3. Set `max_host_work` for JSON/regex/hash/string/bytes/random-heavy scripts.
4. Set `max_io_bytes` for scripts that can read, write, fetch, print, or exec,
   and tighten the per-operation limits (`http_timeout_ms`, `exec_timeout_ms`,
   `max_*_bytes`) below their defaults where scripts only need small payloads.
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
http_timeout_ms = 10_000
exec_timeout_ms = 30_000
max_http_response_bytes = 8_388_608
max_file_read_bytes = 8_388_608
max_exec_output_bytes = 1_048_576

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
- exact wall-clock accounting for host work (only HTTP and `exec` have
  wall-clock timeouts);
- deadlines for file reads and AWS calls: they can wait in the I/O queue and
  run for as long as the operation takes;
- more than 256 blocking I/O requests running at once in one runtime: beyond
  that, requests queue (HTTP deadlines still apply).

These are explicit boundaries of the current runtime contract, not accidental
omissions.
