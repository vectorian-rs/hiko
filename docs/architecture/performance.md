# Performance Baseline

Measured numbers for the VM and the threaded runtime, and how to reproduce
them. Numbers depend on the machine and allocator; rerun the script before
comparing.

## How to measure

```bash
scripts/bench.sh target/bench.md      # RUNS=n for more runs, HIKO_BIN=path to skip the build
```

The script needs [hyperfine](https://github.com/sharkdp/hyperfine). It runs:

| Benchmark | What it measures |
|---|---|
| `process_creation_cost` example | Time and allocations to create a child VM, with and without a filesystem policy. |
| `benchmarks/{ack,tak,nqueens,primes,ec_ack,ec_fib,early_exit}.hml` | Single-process interpreter speed. |
| `benchmarks/global_builtin.hml` | A loop calling a builtin through a global 3,000,000 times. |
| `benchmarks/list_length.hml` | The length of a 100,000-element list, 100 times: constructor pattern matches. |
| `benchmarks/spawn_await.hml` | Spawn and await 6,000 trivial children through the threaded runtime, at 1 and 8 workers. |
| `benchmarks/fanout_cpu.hml` | 64 children computing `fib 22`, at 1, 2, 4 and 8 workers. |
| `benchmarks/fanout_read.hml` | 1,000 children each reading `README.md`, at 1, 2, 4 and 8 workers. |
| `benchmarks/read_sequential.hml` | One process reading `README.md` 1,000 times in sequence: the round trip of an async I/O request. |
| `benchmarks/fanout_read_sequential.hml` | 32 children each reading `README.md` 100 times in sequence, at 1, 2, 4 and 8 workers. |
| `benchmarks/spawn_shared.hml` | One 10,000-element list sent to 200 children, as a capture and as a top-level binding. |
| `benchmarks/string_concat.hml` | 200,000 `^` and one `string_join`: the cost of building new strings. |
| `benchmarks/string_args.hml` | A 10,000-character string passed to builtins 400,000 times. |
| `benchmarks/spawn_strings.hml` | A 100,000-character string sent to 200 children and back. |
| `benchmarks/fanout_hash.hml` | Walk `.git` once, then 32 children each read and BLAKE3-hash a 32nd of the files, at 1, 2, 4 and 8 workers. |
| `examples/benchmark.hml` vs `examples/benchmark.py` | Walk `.git` and hash every file, against Python. |
| `fs_open_cost` example | Open and read every file under a directory with `std::fs`, cap-std, and `O_RESOLVE_BENEATH`. |

In a git worktree, where `.git` is a file, the `.git` workloads run on a copy
of the repository's git directory in `target/bench-git`. `benchmark.hml` needs
every filesystem builtin, so it runs under the default policy.

Scripts run under the `benchmark` policy (`policies/benchmark.policy.toml`),
which has no fuel limit. `hiko run --workers N` sets the threaded runtime's
worker count; the default is 1.

## Baseline (2026-10-31)

Apple M1 Max, 10 cores, macOS (Darwin arm64), release build, hyperfine means
of 10 runs. "Before" is `main` at `d867aa8`; "after" is PR #104.

### Process creation

`cargo run -p hiko-vm --example process_creation_cost --release`:

| Operation | Before | After |
|---|---|---|
| `VM::create_child` | 11.8 µs, 151 allocations | 0.33 µs, 7 allocations |
| `VM::create_child` with a filesystem policy (root plus five per-builtin folders) | 85.4 µs, 232 allocations | 0.33 µs, 7 allocations |
| Spawn path, 0 captures | 11.9 µs, 152 allocations | 0.39 µs, 8 allocations |
| Spawn path, 4 captures | 12.4 µs, 177 allocations | 1.06 µs, 33 allocations |

The 4-capture line includes cloning the four `SendableValue`s on every
iteration, which the benchmark does to build each child's input.

### Threaded runtime, one worker

| Workload | Before | After |
|---|---|---|
| `spawn_await.hml` (6,000 children) | 429 ms | 22 ms |
| `spawn_await.hml` under `software-developer-role` | 1,698 ms | 42 ms |
| `fanout_read.hml` (1,000 reads) | 85 ms | 39 ms |
| `fanout_cpu.hml` | 411 ms | 404 ms |

Before, every spawn rebuilt the builtin table and reopened every allowed
folder; `software-developer-role` allows many folders, so it paid most. CPU-bound
work did not change.

### Worker scaling (after)

| Workers | `fanout_cpu.hml` | `fanout_read.hml` | `spawn_await.hml` |
|---|---|---|---|
| 1 | 402 ms | 39 ms | 22 ms |
| 2 | 214 ms | 40 ms | 28 ms |
| 4 | 114 ms | 40 ms | 39 ms |
| 8 | 81 ms | 41 ms | 87 ms |

- **CPU fan-out scales** to 4.9x at 8 workers.
- **`fanout_read` does not scale.** Each read is a small cached file, so the
  run is dominated by the root process spawning and awaiting children one at
  a time.
- **`spawn_await` gets slower with more workers.** The root spawns 6,000
  trivial children in sequence, so there is no parallel work. More workers
  only add hand-offs: at 8 workers, 150 ms of the time is system time, spent
  waking a worker on each enqueue. A profile shows the waits on the scheduler
  queue's mutex and condition variable.

  A work-stealing scheduler (#47) would not help this case, which has nothing
  to steal, and `fanout_cpu` shows no queue bottleneck, so it is not
  implemented.

### Values sent to many children (#40)

`spawn_shared.hml`, and the capture half of it on its own:

| Workload | Before | After |
|---|---|---|
| 200 children capturing one 10,000-element list | 132 ms | 55 ms |
| `spawn_shared.hml` (capture and top-level binding) | 213 ms | 110 ms |

Before, `spawn` serialized each capture again for every child, and a child
reading an inherited top-level binding deep-copied the shared snapshot before
deserializing it. Now the parent's heap caches the serialized form of each
heap value it sends (cleared on collection, when a closure's captures are
rewritten, and when `max_message_bytes` changes; capped at 16 MiB), and
deserialization reads the shared value in place. What remains is each child
building its own copy in its own heap.

### Filesystem reads on macOS (#82)

`cargo run --release -p hiko-vm --example fs_open_cost -- target/bench-git`,
5,880 files:

| Open + read | Time |
|---|---|
| `std::fs::read` | 102 ms |
| cap-std `Dir::open` | 446 ms |
| `openat` with `O_RESOLVE_BENEATH` | 104 ms |

On macOS, cap-std checks every path component in user space, with several
syscalls each. macOS 15 enforces the same rule in the kernel with
`O_RESOLVE_BENEATH`, so file reads through a filesystem policy now use it,
after a probe shows the kernel enforces it, and fall back to cap-std on any
error. `test_resolve_beneath_matches_cap_std` checks both refuse the same
escapes. (On Linux, cap-std already uses `openat2`.)

`examples/benchmark.hml` (walk and hash the 5,880 files):

| | Time |
|---|---|
| Hiko before | 560 ms |
| Hiko after | 217 ms |
| Python | 196 ms |

Hiko is within 1.11x of Python; in the full `bench.sh` run below, 216 ms
against 199 ms (1.08x).

### File-hash fan-out (after)

`fanout_hash.hml`, from the full `bench.sh` run at commit `9fd66a4`. The
machine was loaded by other work during that run (load average about 10 on 10
cores), so compare rows with each other, not with the tables above.

| Workers | Time | Speedup |
|---|---|---|
| 1 | 207 ms | 1.0x |
| 2 | 137 ms | 1.5x |
| 4 | 111 ms | 1.9x |
| 8 | 100 ms | 2.1x |

About 50 ms of each run is serial work in the root process (walking `.git`,
splitting the list, spawning). With eight passes over the files per child,
so that the serial part matters less, 1 to 8 workers goes from 1.30 s to
0.50 s (2.6x). Plain Rust threads reading the same files on the same machine
reach 2.4x to 3x, so the parallel part scales as far as the OS lets it.

### I/O completion latency (after)

In the threaded runtime, `read_file`, HTTP requests and `sleep` are async: the
process parks, the I/O backend runs the request on its thread pool, and the
runtime's monitor thread queues the process again when the result arrives.
The monitor used to sleep 1 ms between polls of the backend, so every request
also waited for the next tick, about 1.5 ms on macOS. The backend now wakes
the monitor when it stores a result.

Measured at commit `8be4802`, load average about 12 on 10 cores; "before" is
the same branch without that commit.

| Workload | Before | After |
|---|---|---|
| `read_sequential.hml` (1,000 reads in one process) | 1.52 s | 39 ms |

`fanout_read_sequential.hml`:

| Workers | Before | After |
|---|---|---|
| 1 | 152 ms | 124 ms |
| 2 | 153 ms | 122 ms |
| 4 | 154 ms | 118 ms |
| 8 | 163 ms | 110 ms |

`fanout_read.hml`, `fanout_hash.hml`, `fanout_cpu.hml` and `spawn_await.hml`
are unchanged within noise. `fanout_hash` uses `read_file_bytes`, which is
not async; `fanout_read` keeps 32 requests in flight, so the delay overlapped.

### What the worker count scales

Async I/O does not run on the runtime's workers. Each process tree may have
`max_concurrent_io` requests in flight (32 by default) on the I/O backend's
pool, so I/O is already concurrent with one worker, and the worker count only
scales the work between requests:

- **CPU-bound fan-out** scales to 4.9x at 8 workers (`fanout_cpu`).
- **File-hash fan-out**, whose reads and hashing run on the workers, scales to
  2.1x, as far as plain Rust threads get on the same files (2.4x to 3x).
- **Async read fan-outs** (`fanout_read`, `fanout_read_sequential`) are
  concurrent at one worker and stay flat. Their time is now kernel time: 32
  backend threads opening the same file use about 800 ms of system time in a
  120 ms run.

### Shared strings and bytes (#12)

Strings and bytes are `Arc<str>` / `Arc<[u8]>` in the heap, the same types
`SendableValue` uses, so crossing a process boundary shares the payload
instead of copying it twice. `main` at `f322f68` against PR #125, hyperfine
means of 15 to 20 runs:

| Benchmark | Before | After |
|---|---|---|
| `spawn_strings.hml` | 17.3 ms | 11.6 ms |
| `string_concat.hml` | 302 ms | 301 ms |
| `string_args.hml` | 272 ms | 275 ms |

- **Building strings.** An `Arc<str>` cannot be built in place without
  `unsafe`, so converting a new `String` copies it again. A first version
  that did this everywhere made `string_concat` about 10% slower. `^` now
  builds into a buffer the VM reuses, `int_to_string` formats on the stack,
  and `substring` copies one slice, so each allocates once, as before. Other
  builtins still convert a `String`; their cost is dominated by the work that
  produced it.
- **Builtin arguments.** `string_length` and `substring` already read their
  argument in place, so `string_args` does not change. The helpers that did
  copy (`extract_string_arg`, the filesystem `string_arg`) serve paths and
  URLs, where the copy is small next to the I/O.
- **No regressions** beyond noise in the same A/B run: `ack`, `tak`,
  `nqueens`, `ec_fib`, `spawn_shared`, `spawn_await`, `fanout_hash` at 8
  workers, and `benchmark.hml`.

### Function calls (#131)

`main` at `d375a47` against PR #131, side by side, hyperfine means of 10 runs
under the `benchmark` policy:

| Benchmark | Before | After | Speedup |
|---|---|---|---|
| `ack.hml` | 2.79 s | 1.21 s | 2.3x |
| `tak.hml` | 664 ms | 261 ms | 2.5x |
| `nqueens.hml` | 31.2 ms | 16.7 ms | 1.9x |
| `ec_ack.hml` | 240 ms | 133 ms | 1.8x |
| `early_exit.hml` | 506 ms | 332 ms | 1.5x |
| `string_concat.hml` | 210 ms | 135 ms | 1.6x |
| `ec_fib.hml` | 972 ms | 695 ms | 1.4x |
| `fanout_cpu.hml` | 337 ms | 253 ms | 1.3x |
| `string_args.hml` | 186 ms | 159 ms | 1.2x |
| `spawn_await.hml`, `spawn_shared.hml`, `spawn_strings.hml`, `fanout_hash.hml`, `primes.hml` | | | within noise to 1.2x |

What each change removed from a call such as `ack (m - 1, 1)`:

- **A heap tuple.** A top-level function whose clauses take one tuple
  pattern gets a flat entry with one parameter per component, and a call
  with a tuple expression passes the components directly. The ordinary
  entry remains for tuple values and for the function used as a value.
- **The generic call path.** Calls between members of a top-level `fun`
  group are `CallDirect`/`TailCallDirect`, instead of `GetGlobal` and `Call`
  with a callee check, a closure lookup and a captures clone.
- **Allocations.** `values_equal` no longer allocates to compare two scalars
  (`m = 0`), and direct calls share one empty captures `Arc`.
- **Repeated lookups.** Reading an operand finds the current frame and its
  code once instead of three times.

Fusing the opcode fetch at the top of the dispatch loop into the same helper
made every benchmark about 15% slower and was not kept.

### Interpreter (after, one process)

| Benchmark | Time |
|---|---|
| `ack.hml` (3, 9) | 3.09 s |
| `ec_fib.hml` (30) | 1.03 s |
| `tak.hml` (24, 16, 8) | 0.72 s |
| `early_exit.hml` | 0.59 s |
| `ec_ack.hml` (3, 7) | 0.25 s |
| `nqueens.hml` (8) | 35 ms |
| `primes.hml` | 5 ms |

These did not change in PR #104; see below for PR #124.

### Bytecode dispatch (PR #124)

Two per-instruction costs that the compiler can resolve ahead of time:

- **Global slots.** `GetGlobal` and `SetGlobal` took a constant-pool index of
  the global's name, and dispatch hashed the name on every execution. The
  compiler now gives each global a slot (`CompiledProgram::globals`), and a
  loaded slot is an index and a branch. Children still resolve builtins and
  inherited top-level values on first use.
- **One-instruction tag test.** A constructor pattern tested its tag with
  `GetTag; Const; Eq; JumpIfFalse`. It is now one `JumpIfNotTag`, and `GetTag`
  is gone.

Apple M1 Max, load average about 3, hyperfine means; "before" is `main` at
`f322f68`.

| Benchmark | Before | After |
|---|---|---|
| `global_builtin.hml` | 1.054 s | 0.896 s (-15%) |
| `list_length.hml` | 4.44 s | 3.53 s (-21%) |
| `early_exit.hml` | 0.762 s | 0.657 s (-14%) |
| `fanout_cpu.hml`, 1 worker | 520 ms | 452 ms (-13%) |
| `nqueens.hml` | 45 ms | 42 ms (-7%) |
| `ec_fib.hml` | 1.300 s | 1.246 s (-4%) |
| `ack.hml`, `tak.hml`, `ec_ack.hml`, `primes.hml` | | unchanged within 1% |
| `spawn_await.hml`, `spawn_shared.hml` | | unchanged within noise |

`global_builtin` gained all of its 15% from the global slots and
`list_length` all of its 21% from `JumpIfNotTag` (measured on the
intermediate commits). A spawned child makes one allocation fewer (spawn path:
7 instead of 8; with 4 captures 25 instead of 26), because its global table is
no longer preallocated.
