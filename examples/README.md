# Examples

Run an example from the repository root:

```sh
hiko run examples/hello.hml
```

Some examples use regex, JSON or dates, which the default build leaves out.
Build with every builtin to run them all:

```sh
cargo build --release -p hiko-cli --features builtin-all
```

`tools/run_all.hml` runs every example except the ones that must fail. Those
are checked by `crates/hiko-cli/tests/examples.rs`. Examples that import named
modules (`import Std.List`) fetch them from the registry in `hiko.toml`; serve
it locally with:

```sh
python3 -m http.server 8000 -d libraries --bind 127.0.0.1
```

## Language

| File | Shows |
| --- | --- |
| `hello.hml` | Hello, world. |
| `factorial.hml` | Clausal function definitions. |
| `fibonacci.hml` | Recursion. |
| `fizzbuzz.hml` | A tuple pattern match. |
| `closures.hml` | Closures and higher-order functions. |
| `list_ops.hml` | `map`, `filter` and folds written by hand. |
| `quicksort.hml` | Quicksort with pattern matching. |
| `binary_tree.hml` | A binary search tree datatype. |
| `expr_eval.hml` | A small expression evaluator. |
| `word_count.hml` | Counting words with `split`. |
| `option.hml` | An option datatype written by hand. |
| `either.hml` | An either datatype for errors. |
| `error_handling.hml` | Recoverable errors with `Std.Result`. |
| `generator.hml` | Generators built from closures. |
| `state.hml` | State passed along instead of mutated. |
| `string_ops.hml` | `trim`, `split`, `substring` and friends. |
| `string_utils.hml` | Case conversion, prefixes and suffixes. |
| `numeric_modules.hml` | `Std.Int32`, `Std.Word32` and `Std.Float32`. |
| `stdlib_test.hml` | `Std.List` folds and helpers. |
| `testing.hml` | `assert`, `assert_eq` and `Result` in tests. |

## Modules

| File | Shows |
| --- | --- |
| `import_test.hml` | Local file inclusion with `use`; a repeated `use` is included once. |
| `math.hml` | Helpers included by `import_test.hml`. |
| `cycle_a.hml`, `cycle_b.hml` | Must fail: two files that `use` each other. |

## Effects

| File | Shows |
| --- | --- |
| `effects_overview.hml` | Provider and abort clauses. |
| `effects_and_io.hml` | Effects next to ordinary I/O builtins. |
| `http_effect_wrapper_test.hml` | One HTTP effect with a mock and a real handler. The real handler needs the local module server. |

## Runtime and host builtins

| File | Shows |
| --- | --- |
| `file_io.hml` | Writing and reading a file under `target/`. |
| `io_builtins.hml` | I/O through builtins, not effects. |
| `dir_walk.hml` | A recursive walk with `list_dir`. |
| `glob_walk.hml` | `glob` and `walk_dir`. |
| `pipeline.hml` | A data pipeline that replaces a shell pipe. |
| `regex_demo.hml` | Regular expression matching and replacement. |
| `json.hml` | JSON path builtins. |
| `json_typed.hml` | JSON as an algebraic datatype. |
| `env_and_time.hml` | `getenv` and the clock. |
| `exec_demo.hml` | How `exec` is enabled by policy. |
| `random_demo.hml` | OS entropy and a seeded generator. |
| `rng_demo.hml` | Deterministic random numbers. |
| `spawn_stress.hml` | Spawning and awaiting many isolated processes. |

## Limits and policy

| File | Shows |
| --- | --- |
| `work_budget_demo.hml` | The work budget, with the `work-demo-tight` and `work-demo-enough` policies. |
| `should_fail_io_*.hml` | Must fail under the `should-fail-io` policy: each I/O builtin hits a 4-byte I/O budget. `should_fail_io_fixture.txt` is the file they read. |

## Benchmarks

| File | Shows |
| --- | --- |
| `benchmark.hml`, `benchmark.py` | Walk `.git` and hash every file, in Hiko and in Python. `scripts/bench.sh` runs both. |
