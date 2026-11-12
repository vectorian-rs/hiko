# Performance Baseline

Measured numbers for the VM and the threaded runtime, and how to reproduce
them. Numbers depend on the machine and allocator; rerun the script before
comparing.

## How to measure

```bash
bun scripts/bench.ts target/bench.md      # RUNS=n for more runs, HIKO_BIN=path to skip the build
```

The runner uses Bun (pinned in `mise.toml`) and
[hyperfine](https://github.com/sharkdp/hyperfine). It runs:

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

## Exact-commit VM gate

```bash
bun scripts/vm-gate.ts <base-ref> <candidate-ref> [--history path] [--pairs 11]
```

This separate gate does not change `compare.ts`. It resolves both refs once,
then builds counter-free release binaries with default features and with
`register-vm` from one detached worktree per commit into a unique target per
profile. `HIKO_BIN`, injected
Cargo target/features/flags, and power overrides are refused, not fallbacks.
The repository's pinned Rust toolchain (`scripts/common.ts`), tool paths,
build argv, environment and source
hashes are retained. Builds are locked/offline, use one Cargo job and an isolated
Cargo home sharing downloaded source caches, not ambient configuration or
credentials. Compiler artifact messages verify effective VM features, not just
requested flags. Unsupported effective Cargo configuration is an explicit error.

The ordered HML corpus covers integer recursion, tuple/closure fallback and
string concatenation (`compare/fib`, `compare/tak`, `local_rec`,
`string_concat`). Its configuration, policy and transitive library inputs are
frozen once from the candidate commit, used for both commits, and hashed. Every invocation, including untimed
metrics and rechecks, must exit successfully and match the complete stdout
bytes. Stderr is drained and retained separately, never joined to stdout.

The initial supported host is macOS. Successful recognized `pmset` AC proof is
required before builds and around every sample. One-minute load must be finite,
nonnegative and strictly below `min(4, max(1, CPU-count / 2))`. Before and after
builds, load may settle for at most 60 monotonic seconds at a recorded fixed
interval. Probes have deadlines and timed-out subprocesses are killed and
reaped. Unsupported OS, unavailable/unknown power, invalid load, battery power,
timeout or sample contamination is unmet evidence, with no bypass. Boundary
observations do not prove uninterrupted AC or low load between probes.

At least 11 complete AB/BA pairs are timed per benchmark/profile. Durations are
positive integer monotonic nanoseconds. A regression is the exact comparison
`100 * median(candidate) > 101 * median(base)`; equality is allowed. Only a
valid measured regression gets one fresh complete recheck for that cell. Both
attempts remain separate: second success is `pass-with-recheck`, second
regression fails. Errors/refusals/output mismatches never trigger a performance
retry; no pooling, dropped samples or third attempt is permitted.

Separate untimed default/register profiles add only `heap-metrics`. They
require successful exit and a fresh schema 1 artifact with matching nonce and
profile, complete balanced heap lifecycle and checked safe-integer totals.
See the [metric contract](execution-bounds.md#untimed-heap-metrics): allocation
counts and maximum per-heap estimated peaks are not RSS or simultaneous tree
peaks. Historical commits without the exporter remain explicitly incomplete;
no source patching or zero-filled metrics may produce an overall PASS.

Schema-versioned history retains all samples/order, probes, attempts, metrics,
failures and provenance before owned worktrees/targets are removed. Calls and
probes are appended once to a write-once journal as they complete; the full
snapshot is published at phase boundaries and termination, keeping history I/O
linear over a run. History write failures exit nonzero and preserve the
recovery snapshot and journal. Choose a fresh history
path; an existing destination is refused rather than overwritten. Large compiler
transcripts are retained once as base64 sidecars under `<history>.artifacts`,
with paths and byte hashes in the history. Preserve those sidecars with the JSON.
The gate changes no runtime semantics, does not gate CI wall-clock time, and
does not supply other required workspace checks or reviews.

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

Hiko is within 1.11x of Python; in the full benchmark run below, 216 ms
against 199 ms (1.08x).

### File-hash fan-out (after)

`fanout_hash.hml`, from the full benchmark run at commit `9fd66a4`. The
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

### Regex cache (#135)

`regex_match` and `regex_replace` compiled their pattern on every call. A
cache shared by the process tree keeps up to 32 compiled patterns (each
under 256 KiB of program and 256 KiB of lazy DFA). Host work is charged as
before, so a cache hit costs the same budget as compiling.

| Benchmark | Before | After |
|---|---|---|
| `regex_loop.hml` (12,000 calls, one pattern) | 163 ms | 9.3 ms |

The first version returned clones of the cached `Regex`. A clone gets its
own search cache and allocates it again on its first search, so cache hits
were barely faster than compiling (53 ms). Entries are now `Arc<Regex>`.

### Local recursive functions (#135)

A function defined in `let` or `local` can capture variables, so the direct
calls of #131 do not apply to it. Its calls to itself now compile to
`CallSelf`/`TailCallSelf`, which reuse the running function and the current
frame's captures instead of looking the closure up.

| Benchmark | Before | After |
|---|---|---|
| `local_rec.hml` | 563 ms | 499 ms |

Local functions are not flattened like top-level ones, so `go (i, acc)`
still builds a tuple; that is most of what remains. The repo has few local
recursive functions (none in `libraries/`), since functions inside a
`structure` compile as top level.

Also measured in #135 and not kept: decoding opcodes from a 256-entry table
instead of `Op::try_from`'s `match`. It made `ack`, `tak`, `early_exit` and
`ec_ack` 2-3% faster and `list_length` 4.5% slower, repeatably.

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

### Compare and branch (PR #132)

A condition such as `if n < 2` compiled to `LtInt`, which pushed a `Bool`,
and `JumpIfFalse`, which popped it. Six instructions now compare and jump in
one dispatch: `JumpIfNotEq` and `JumpIfEq` (structural equality, also used
for literal patterns) and `JumpIfNot{Lt,Gt,Le,Ge}Int`.

Load average about 4 to 10, 15 runs each, so the table gives minimums;
medians moved the same way. "Before" is `main` at `d375a47`.

| Benchmark | Before | After |
|---|---|---|
| `ack.hml` | 2.80 s | 2.61 s (-6.8%) |
| `early_exit.hml` | 509 ms | 480 ms (-5.6%) |
| `ec_fib.hml` | 975 ms | 926 ms (-5.0%) |
| `global_builtin.hml` | 651 ms | 621 ms (-4.7%) |
| `tak.hml` | 668 ms | 654 ms (-2.1%) |
| `list_length.hml` | 2.63 s | 2.58 s (-2.1%) |
| `fanout_cpu.hml`, `spawn_await.hml`, `nqueens.hml`, `primes.hml`, `ec_ack.hml` | | unchanged within noise |

### Against Lua and Python (PR #146)

The programs in `benchmarks/compare/` do the same work, in the same order,
in Hiko, Lua and Python (rules in `benchmarks/compare/README.md`). Every
implementation prints the same checksum. `bun scripts/compare.ts` checks the
checksums, runs hyperfine, and writes this table.

Apple M1 Max on AC power, load average about 5 to 7 (a browser tab used one
core), 10 runs each, hyperfine means. Hiko at `65f955e` (main after #139), Lua
5.5.0, Python 3.13.10, LuaJIT 2.1 (JIT on, as a ceiling).

| Program | Hiko | Lua | Python | LuaJIT | Hiko / Lua | Hiko / Python |
|---|---|---|---|---|---|---|
| `binary_trees` (max depth 14) | 1454 ms | 543 ms | 300 ms | 136 ms | 2.7x | 4.8x |
| `fib` (32) | 472 ms | 138 ms | 240 ms | 17 ms | 3.4x | 2.0x |
| `list` (300 repetitions) | 1755 ms | 124 ms | 172 ms | 19 ms | 14.1x | 10.2x |
| `mandelbrot` (300 x 300, 50 iterations) | 636 ms | 105 ms | 215 ms | 15 ms | 6.1x | 3.0x |
| `nbody` (20,000 steps) | 642 ms | 97 ms | 109 ms | 17 ms | 6.6x | 5.9x |
| `queens` (10) | 351 ms | 51 ms | 102 ms | 11 ms | 6.9x | 3.4x |
| `sieve` (15,000) | 608 ms | 343 ms | 156 ms | 100 ms | 1.8x | 3.9x |
| `tak` (24, 16, 8) | 257 ms | 54 ms | 82 ms | 12 ms | 4.7x | 3.1x |

Hiko is 2 to 14 times slower than Lua 5.5 and 2 to 10 times slower than
Python on every program. Start-up is not the cause: a one-line program takes
4.9 ms in Hiko (lex, parse, type check, compile, verify, run), 3.1 ms in Lua
and 14.4 ms in Python.

Where the time goes (macOS `sample`, 1 s of each run, share of the VM
thread's samples by the function on top of the stack):

| Program | `dispatch` | `read_u16` | `Op::try_from` | stack index helpers | GC and allocation | building data values (`SmallVec::extend`) |
|---|---|---|---|---|---|---|
| `list` | 48% | 18% | 14% | 8% | 1% | - |
| `binary_trees` | 46% | 7% | 11% | 6% | 13% | 9% |
| `sieve` | 43% | 12% | 8% | 10% | 7% | 12% |
| `nbody` | 50% | 10% | 7% | 11% | 2% | 7% |

The gap is interpretive overhead, not memory management. Even on
`binary_trees`, GC and allocation take about 13%. `read_u16`, `Op::try_from`
and the stack index helpers are out-of-line calls on every instruction: 20 to
40% of the time. `list` is the worst case because it is almost only calls,
pattern matches and local reads, each a few instructions with operands.
Next steps, in order of cost: inline those helpers, then measure
LTO/`codegen-units = 1`/PGO on this suite, then larger dispatch changes.

### Builtin dispatch by kind (PR #147)

Before a builtin ran, `call_builtin` compared its id with up to 15 builtins
the VM handles itself (print, exec, the process builtins, sleep, HTTP,
`read_file`). Each builtin now has a dispatch kind stored when it is
registered, and a plain builtin takes one branch to its function.

The machine was busy, so the table gives minimums over 40 runs; user time
moved the same way. "Before" is the V branch at `8b06637`, built with
`builtin-all`.

| Benchmark | Before | After |
|---|---|---|
| `global_builtin.hml` | 291.0 ms | 276.9 ms (-4.8%) |
| `string_concat.hml` | | unchanged within noise |

### Release profile: fat LTO and one codegen unit (PR #149, not kept)

`[profile.release]` with `lto = "fat"` and `codegen-units = 1`, against
Cargo's defaults (no cross-crate LTO, 16 codegen units), both built from
`main` after #148. Interleaved, 10 rounds each, on AC, load average about 3
(a first run at load 10 to 30 pointed the same way).

| Benchmark | Default | Fat LTO, 1 unit | Change |
|---|---|---|---|
| `compare/sieve` | 582 ms | 668 ms | +15% |
| `ack` | 1017 ms | 1068 ms | +5% |
| `early_exit` | 301 ms | 316 ms | +5% |
| `global_builtin` | 284 ms | 296 ms | +4% |
| `compare/fib` | 453 ms | 470 ms | +4% |
| `compare/binary_trees` | 1408 ms | 1460 ms | +4% |
| `compare/mandelbrot` | 602 ms | 624 ms | +4% |
| `compare/nbody` | 616 ms | 636 ms | +3% |
| `tak` | 250 ms | 258 ms | +3% |
| `compare/tak`, `compare/queens`, `list_length` | | | +2.5% |
| `compare/list` | 1677 ms | 1698 ms | +1% |
| `ec_fib`, `string_concat` | | | -1%, 0% |

Medians; the minimums move the same way. The binary is 28% smaller
(6,449,888 to 4,658,512 bytes), but 13 of 15 programs are slower: more
inlining into the very large `dispatch` function likely worsens register
allocation in the hot loop. Not kept. Inlining the specific hot helpers
(review items 2 and 3 in `docs/rnd/2026-11-vm-performance-review.md`) is
still worth measuring on its own, and thin LTO or PGO separately.

### Building data values from a slice (PR #149)

`MakeTuple`, `MakeData` and `MakeCons` collected their fields with
`self.stack.drain(start..).collect::<Fields>()`, which the profiles showed as
`SmallVec::extend` (7 to 12% on `binary_trees`, `sieve` and `nbody`). They now
copy the slice (`Fields::from_slice`, `Value` is `Copy`) and truncate the
stack; `MakeCons` pops its two values into `Fields::from_buf`. The values
still leave the stack before the allocation, as before.

Interleaved, 10 rounds, on AC, load average 2 to 3; "before" is `main` at
`df8d2a9` (after #148) plus the #149 docs.

| Benchmark | Before | After | Change |
|---|---|---|---|
| `compare/sieve` | 582 ms | 519 ms | -11% |
| `compare/binary_trees` | 1404 ms | 1304 ms | -7% |
| `compare/nbody` | 617 ms | 589 ms | -5% |
| `compare/list`, `queens`, `mandelbrot`, `fib`, `tak`, `list_length`, `nqueens`, `ack`, `early_exit`, `global_builtin` | | | within 1% |

Medians; the minimums agree.

### Inlining `Op::try_from` (PR #149)

The dispatch loop decodes every opcode with `Op::try_from`, which lives in
`hiko-compile`. Without `#[inline]` (and without cross-crate LTO) it is a
function call per instruction; the profiles showed it at 7 to 14%. It is now
`#[inline]`.

Interleaved, 10 rounds, on AC, load average 2.4 to 2.8; "before" is #149
after the data-value change.

| Benchmark | Before | After | Change |
|---|---|---|---|
| `compare/fib` | 456 ms | 423 ms | -7% |
| `global_builtin` | 283 ms | 265 ms | -7% |
| `ack` | 1056 ms | 988 ms | -6% |
| `compare/tak` | 248 ms | 235 ms | -5% |
| `early_exit` | 311 ms | 296 ms | -5% |
| `compare/binary_trees` | 1327 ms | 1259 ms | -5% |
| `compare/nbody` | 586 ms | 567 ms | -3% |
| `compare/queens` | 325 ms | 319 ms | -2% |
| `list_length` | 1205 ms | 1202 ms | 0% |
| `compare/sieve` | 521 ms | 526 ms | +1% |
| `compare/list` | 1676 ms | 1709 ms | +2% |

Medians; the minimums agree. `list` and `sieve` lose 1 to 2%, likely from the
dispatch loop's code layout; the rest gain up to 7%.

### Inlining the operand and stack helpers (PR #149)

`read_operand` (and `read_u8`, `read_u16`, `read_i16`), `push`, `pop`,
`current_frame_index`, `checked_stack_index`, `local_stack_index`, `proto`
and `chunk_for_checked` are now `#[inline(always)]`, and the errors they
return are built in `#[cold] #[inline(never)]` functions, so the `format!`
calls no longer make them too large to inline. Error messages are unchanged.
A later commit in the same PR gave `stack_start_for_arity` and
`callee_pos_for_arity` the same treatment; the table below was measured
before it.

Interleaved, 10 rounds, on AC, load average 2.4 to 3.5; "before" is #149
with `Op::try_from` inlined.

| Benchmark | Before | After | Change |
|---|---|---|---|
| `compare/list` | 1743 ms | 1403 ms | -20% |
| `compare/nbody` | 572 ms | 464 ms | -19% |
| `list_length` | 1216 ms | 1006 ms | -17% |
| `compare/sieve` | 534 ms | 464 ms | -13% |
| `compare/queens` | 322 ms | 287 ms | -11% |
| `compare/tak` | 240 ms | 216 ms | -10% |
| `compare/binary_trees` | 1280 ms | 1214 ms | -5% |
| `compare/fib`, `ack`, `global_builtin` | | | within 1% |
| `early_exit` | 294 ms | 303 ms | +3% (load 3.5) |

Medians; the minimums agree. Unlike whole-program LTO, which inlined
everything into `dispatch` and made it slower, this inlines only the small
helpers on the hot path.

### Against Lua and Python, after #149 steps 2 and 3

The cross-language runner, 10 runs each, on AC, load average about 3.5, at
`026f91b`. "#146" is the Hiko time in the first comparison above.

| Program | Hiko (#146) | Hiko | Lua | Python | LuaJIT | Hiko / Lua | Hiko / Python |
|---|---|---|---|---|---|---|---|
| `binary_trees` | 1454 ms | 1216 ms | 496 ms | 292 ms | 133 ms | 2.5x | 4.2x |
| `fib` | 472 ms | 433 ms | 136 ms | 235 ms | 18 ms | 3.2x | 1.8x |
| `list` | 1755 ms | 1380 ms | 123 ms | 169 ms | 18 ms | 11.3x | 8.2x |
| `mandelbrot` | 636 ms | 525 ms | 125 ms | 207 ms | 12 ms | 4.2x | 2.5x |
| `nbody` | 642 ms | 464 ms | 95 ms | 109 ms | 18 ms | 4.9x | 4.3x |
| `queens` | 351 ms | 286 ms | 50 ms | 101 ms | 11 ms | 5.7x | 2.8x |
| `sieve` | 608 ms | 464 ms | 346 ms | 153 ms | 99 ms | 1.3x | 3.0x |
| `tak` | 257 ms | 233 ms | 67 ms | 83 ms | 13 ms | 3.5x | 2.8x |

Hiko is 8 to 28% faster than at #146, and now 1.3 to 11x slower than Lua 5.5.

### Review fixes (PR #149)

After the measurements above, #149 inlined `stack_start_for_arity` and
`callee_pos_for_arity`, made `MakeCons` check the stack before taking its
values, and checked `ip + N` for overflow in `read_operand`. Interleaved, 8
rounds, on AC, load average 5 to 6.5: `list`, `fib`, `tak`, `nbody` within
1%; `binary_trees`, `ack` +1 to 2%; `queens`, `sieve` +2%. Kept: the
`MakeCons` and `read_operand` changes are correctness fixes.

### Keeping the current function's code in `dispatch` (PR #159, #151)

`dispatch` looked up the frame's chunk for every instruction
(`chunk_for_checked`), and every operand read looked up the frame and chunk
again (`read_operand`). It now holds the current function's code slice,
reloading it only when the frame's function changes, and reads operands from
it. The protos and main chunk are `Arc`-cloned into locals so the slice does
not borrow `self`.

Interleaved, 10 rounds, on AC, load average 4 to 8 (medians; the minimums
agree); "before" is #149's head.

| Benchmark | Before | After | Change |
|---|---|---|---|
| `compare/tak` | 218 ms | 190 ms | -13% |
| `global_builtin` | 272 ms | 237 ms | -13% |
| `compare/fib` | 432 ms | 382 ms | -12% |
| `compare/list` | 1357 ms | 1208 ms | -11% |
| `ack` | 1005 ms | 895 ms | -11% |
| `list_length` | 1049 ms | 937 ms | -11% |
| `compare/nbody` | 477 ms | 426 ms | -11% |
| `compare/queens` | 289 ms | 260 ms | -10% |
| `early_exit` | 299 ms | 270 ms | -10% |
| `compare/mandelbrot` | 541 ms | 493 ms | -9% |
| `compare/binary_trees` | 1249 ms | 1247 ms | 0% |
| `compare/sieve` | 488 ms | 508 ms | +4% |

### Allocation fixes (PR #153 work, stacked on #159)

Interleaved, 8 rounds each, on AC, load average 2.5 to 3.5; each step
against the one before.

- **String equality without a worklist.** `values_equal` allocated a `Vec`
  worklist for every comparison of two heap values; two strings now compare
  directly, and the worklist is an inline `SmallVec`. `string_concat` -1%,
  `string_args` 0%. Kept: it removes an allocation at no cost.
- **String constants in per-function rows.** The string-constant cache was a
  SipHash `HashMap<(usize, usize), GcRef>` looked up on every string
  `Const`; it is now a vector of rows. `string_concat` 130 to 120 ms (-7.5%;
  minimum -6.6%), `string_args` 0%. Kept.
- **GC threshold floor 1024 to 16K objects.** Not kept: `binary_trees`,
  `list`, `sieve`, `nbody`, `list_length`, `string_concat` and `spawn_await`
  all within 1.7%, and a higher floor lets each process hold more garbage.
- **Chunked heap-budget reservation.** Not done: the atomic charge it would
  avoid runs only when `max_memory_bytes` is set, which no benchmark does,
  and reserving in chunks would let one process take budget its siblings
  could use near the limit.

### Untimed heap estimates (#232)

Use the default-off `heap-metrics` feature only in separate untimed samples.
Sum cumulative successful GC-object allocations and report the maximum of
per-heap estimated retained-byte peaks; configuration, schema, validation,
accounting caveats and completeness are defined by the
[heap-metrics contract](execution-bounds.md#untimed-heap-metrics).
`opcode-stats` is separate instrumentation, not heap metrics or completion
proof.

### Arena spike/churn cadence (#201)

Completed sweeps rebuild every vacant slot once. Unguarded heaps amortize a
retained arena with `max(legacy, arena.len / 2)` allocation cadence and
`max(256, effective / 4)` boundary cadence. Any shared-byte or total/per-kind
host-resource budget present selects legacy cadence dynamically, even with no
local handles; guarded profiles have no spike-amortization timing guarantee.
See the [GC contract](execution-bounds.md#garbage-collection-behavior).

Limit-forced GC prepays abstract work against persistent opcode fuel; ordinary
and boundary GC remain unmetered. Units are conservative work/envelope charges,
not CPU timing, and failed blocks are not completed-work counters. This is not a
moving/resumable collector or a native allocator retry. Performance comparisons
must pair current-baseline libraries/toolchain/profiles, omit setup, and keep
counter-free release timings separate from quota/fuel outcome tests.

### Rooted allocation safepoints (#206)

Allocation preflight is per logical instruction, including the complete recursive
closure group, not per member. Neither the collector nor its cadence changes.
Compare counter-free concatenation/local-recursion runs in matched, interleaved
stack and register-enabled profiles; allocating functions still use fallback.
A translated caller entering that fallback is not an all-translated allocation
claim. Unexplained regressions greater than 1% remain failures, not exemptions.

For memory, use separate matched untimed, single-heap, whole-fixture lifetime
estimated peaks: `abs(Pcandidate - P201) <= E`. `E` is the maximum complete
logical-instruction footprint, including all group members and spilled-field
capacity. Keep setup, roots, policy, limits and the lifetime window identical;
report setup domination and never subtract lifetime peaks or widen the envelope
after observing results. These are neither RSS nor native/reachable-byte proofs.
Missing measurements or divergent outcomes remain unmet acceptance.

### What the VM executes (opcode counts, #152)

Counted with the `opcode-stats` feature (`HIKO_OPCODE_STATS=<path>`) on
`main` plus #149, #159 and #164. "Move" is the share of executed instructions
that only move values between locals and the stack: `GetLocal*`,
`SetLocal*`, `Pop`.

| Program | Executed | Move share | Top opcodes |
|---|---|---|---|
| `compare/tak` | 38.0M | 67% | `GetLocal` 21%, `GetLocal3` 11%, `Pop` 10% |
| `compare/sieve` | 67.7M | 65% | `GetLocal` 44%, `JumpIfNotTag` 9%, `GetLocal3` 7% |
| `compare/nbody` | 72.2M | 65% | `GetLocal` 50%, `GetField` 8%, `GetLocal0` 6% |
| `compare/list` | 231.9M | 63% | `GetLocal` 40%, `GetField0` 15%, `JumpIfNotTag` 10% |
| `compare/queens` | 47.1M | 63% | `GetLocal` 38%, `GetLocal3` 9%, `JumpIfNotEq` 5% |
| `list_length` | 171.2M | 59% | `GetLocal` 35%, `JumpIfNotTag` 12%, `GetLocal2` 6% |
| `compare/mandelbrot` | 89.1M | 55% | `GetLocal` 43%, `Const` 11%, `MulFloat` 10% |
| `compare/binary_trees` | 164.6M | 46% | `GetLocal1` 10%, `GetLocal0` 10%, `Pop` 10% |
| `ack` | 167.5M | 43% | `Const` 20%, `GetLocal2` 13%, `JumpIfNotEq` 10% |
| `compare/fib` | 67.0M | 26% | `GetLocal0` 26%, `Const` 21%, `Return` 11% |

Over all ten programs, 1.12 billion instructions, 54% are moves. The generic
`GetLocal` (2-byte operand) alone is 35 to 50% on most programs: the short
forms cover only slots 0 to 3.

Most frequent consecutive pairs, as a share of all pairs: `GetLocal ->
GetLocal` 6.3%, `GetLocal -> JumpIfNotTag` 4.7%, `JumpIfNotTag -> GetLocal`
4.7%, `GetLocal -> GetField0` 4.4%, `GetLocal0 -> GetLocal1` 4.1%,
`GetField0 -> GetLocal` 3.4%, `GetLocal1 -> GetLocal2` 2.9%, `GetField1 ->
GetLocal` 2.8%, `GetLocal -> GetField1` 2.4%, `Const -> JumpIfNotEq` 2.1%.

### Superinstruction: field of a local (PR #165, #152)

`GetLocal slot; GetField0` and `GetLocal slot; GetField1` (6.8% of executed
pairs) became `GetLocalField0 slot` / `GetLocalField1 slot`. The compiler
fuses them only when no jump lands between the two. Interleaved, 10 rounds,
on AC, load average 3 to 3.7:

| Benchmark | Before | After | Change |
|---|---|---|---|
| `compare/list` | 1163 ms | 997 ms | -14% |
| `list_length` | 885 ms | 809 ms | -9% |
| `compare/sieve` | 405 ms | 382 ms | -6% |
| `compare/nbody` | 395 ms | 382 ms | -3% |
| `compare/queens` | 257 ms | 250 ms | -3% |
| `compare/binary_trees` | 1067 ms | 1042 ms | -2% |
| `compare/tak`, `compare/fib`, `compare/mandelbrot`, `ack` | | | within 1% |

Medians; the minimums agree.

### Superinstruction: tag test on a local (PR #165, #152)

Pattern tag checks were `GetLocal slot; JumpIfNotTag tag` (4.7% of executed
pairs). The compiler now emits `JumpIfLocalNotTag slot tag offset`, which
reads the local in place. Interleaved, 10 rounds, on AC, load average 2 to
3.2; "before" includes the field-of-a-local superinstruction.

| Benchmark | Before | After | Change |
|---|---|---|---|
| `compare/list` | 995 ms | 926 ms | -7% |
| `list_length` | 806 ms | 751 ms | -7% |
| `compare/sieve` | 376 ms | 358 ms | -5% |
| `compare/queens` | 247 ms | 240 ms | -3% |
| `compare/binary_trees` | 1035 ms | 1017 ms | -2% |
| `compare/nbody` | 382 ms | 376 ms | -1% |
| `compare/tak`, `compare/fib`, `ack` | | | 0% |

Medians; the minimums agree.

### Superinstruction: two local reads (PR #165, not kept)

`GetLocal a; GetLocal b` (6.3% of executed pairs, the most frequent) as one
`GetLocalPair a b`. Interleaved, 10 rounds, on AC, load average 1.4 to 2.6:
`compare/mandelbrot` -8%, `compare/nbody` -5%, `compare/queens` -3%,
`list_length` -2%, `compare/list` -1%; but `compare/sieve` +17%,
`compare/binary_trees` +11%, `compare/fib` +8%, `ack` +5%, `compare/tak`
+3%. `fib` and `ack` barely use the pair, so the loss is likely the dispatch
loop's code layout or register allocation rather than the fusion. Reverted.
A register VM (#155) removes these reads without adding opcodes to the
stack VM's loop.

### Against Lua and Python, after #159, #164 and #165

The cross-language runner, 10 runs each, on AC, load average about 2.5, with
#159 (code slice in `dispatch`), #164 (string fixes) and #165's kept
superinstructions (field of a local, tag test on a local). "#146" is the
first comparison above.

| Program | Hiko (#146) | Hiko | Lua | Python | LuaJIT | Hiko / Lua | Hiko / Python |
|---|---|---|---|---|---|---|---|
| `binary_trees` | 1454 ms | 1053 ms | 504 ms | 294 ms | 134 ms | 2.1x | 3.6x |
| `fib` | 472 ms | 391 ms | 137 ms | 241 ms | 18 ms | 2.9x | 1.6x |
| `list` | 1755 ms | 962 ms | 125 ms | 171 ms | 19 ms | 7.7x | 5.6x |
| `mandelbrot` | 636 ms | 466 ms | 105 ms | 210 ms | 12 ms | 4.4x | 2.2x |
| `nbody` | 642 ms | 406 ms | 96 ms | 113 ms | 17 ms | 4.2x | 3.6x |
| `queens` | 351 ms | 250 ms | 51 ms | 101 ms | 11 ms | 4.9x | 2.5x |
| `sieve` | 608 ms | 373 ms | 341 ms | 157 ms | 101 ms | 1.1x | 2.4x |
| `tak` | 257 ms | 195 ms | 54 ms | 82 ms | 12 ms | 3.6x | 2.4x |

Hiko is 17 to 45% faster than at #146, and 1.1x to 7.7x slower than Lua 5.5.

### Caching the chunk, and calls without captures (#151, #152)

Two changes to `dispatch`, each measured against the one before it.
Interleaved A/B (`ab.py`), on AC, load average 2.6 to 3.5. B/A is the ratio
of medians.

- **Chunk cache:** `dispatch` keeps the current function's whole `Chunk`,
  not just its code. `Const` reads the cached constants without looking up
  the function again, and jumps check against the cached code length.
- **Captures:** a frame holds `Captures::Empty` for a call with no captures
  instead of a reference-counted empty slice. `CallDirect`, tail calls and
  returns no longer touch a reference count.

| Program | chunk cache: B/A | captures: B/A |
|---|---|---|
| `compare/fib` | 0.973 | 0.947 |
| `compare/tak` | 0.975 | 0.952 |
| `ack` | 0.972 | 0.962 |
| `compare/queens` | 0.975 | 0.974 |
| `compare/list` | 0.984 | 0.975 |
| `compare/sieve` | 0.999 | 0.973 |
| `compare/binary_trees` | 0.990 | 0.989 |
| `compare/nbody` | 0.993 | 0.986 |
| `compare/mandelbrot` | 1.000 | |
| `early_exit` | 0.976 | |

Together, call-heavy programs are 6 to 8% faster (`fib` 377 to 353 ms,
`tak` 192 to 182 ms), and nothing is slower.

### Register VM, stage 1 (#155, behind `register-vm`)

Functions whose bytecode uses only integer arithmetic and comparisons,
locals, jumps, direct and self calls, tail calls, `Return`, `Panic` and
`Halt` run as register code; everything else stays on the stack VM. See
`docs/rnd/2026-11-register-vm-design.md`. Interleaved A/B against `main`,
10 rounds, on AC, load average 2.3 to 3.2. B/A is the ratio of medians.

| Program | feature off | `register-vm` |
|---|---|---|
| `compare/fib` | 0.971 | 0.781 |
| `compare/tak` | 0.988 | 0.614 |
| `ack` | 0.981 | 1.001 |
| `compare/sieve` | 0.995 | 0.994 |
| `compare/list` | 0.988 | 0.998 |
| `compare/queens` | 0.997 | 0.988 |
| `compare/binary_trees` | 0.982 | 0.988 |
| `compare/mandelbrot` | 0.992 | 1.002 |
| `compare/nbody` | 0.998 | 0.990 |

`fib` and `tak` translate whole and are 22% and 39% faster. `ack` uses `=`,
which stage 1 does not translate, and runs on the stack VM.

Three changes to the stack VM had to be undone because they slowed it with
the feature off, though none of them adds work per instruction:

- `Return` calling a helper shared with the register VM made `fib` about
  55% slower; the stack VM keeps its own copy.
- An always-present `register_code` field on the VM (empty when the feature
  is off) made `sieve` 19% and `binary_trees` 5% slower; the field exists
  only with the feature.
- Moving the stack loop's fuel check after the function-change check made
  no difference once the field was gone, and is kept.
