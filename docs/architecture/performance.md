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
| `benchmarks/spawn_await.hml` | Spawn and await 6,000 trivial children through the threaded runtime, at 1 and 8 workers. |
| `benchmarks/fanout_cpu.hml` | 64 children computing `fib 22`, at 1, 2, 4 and 8 workers. |
| `benchmarks/fanout_read.hml` | 1,000 children each reading `README.md`, at 1, 2, 4 and 8 workers. |
| `examples/benchmark.hml` vs `examples/benchmark.py` | Walk and hash `.git`. Skipped in a git worktree, where `.git` is a file. |

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

These did not change in PR #104. A loop that calls a builtin through a global
spends about 6% of its time on `GetGlobal`, which hashes the global's name on
every execution; resolving globals to slots at compile time would remove that.
