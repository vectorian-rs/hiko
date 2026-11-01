# Builtin Resource Accounting Matrix

This document audits builtin and builtin-adjacent resource accounting as of the
first-pass #63 implementation and the #80 host-resource limits work.

It is intentionally an accounting matrix, not a type reference. For signatures
and user-facing descriptions, see [`builtins.md`](builtins.md). For execution
latency and non-preemptive sections, see
[`../architecture/execution-bounds.md`](../architecture/execution-bounds.md).

## Legend

| Mark | Meaning |
| --- | --- |
| ✅ | Accounted/enforced for this domain. |
| ◐ | Partially accounted, approximate, or covered by a broader/global limit. |
| N/A | Not applicable to this domain. |
| ⚠️ | Known gap or tracked follow-up. |

## Accounting dimensions

| Dimension | Mechanism |
| --- | --- |
| Opcode fuel | VM dispatch fuel / `max_work` / `max_fuel`; every executed opcode consumes fuel. Builtin calls consume opcode fuel for the call dispatch, but native work inside the builtin needs separate accounting. Fuel, host work, I/O bytes and memory are shared by a whole process tree (see [execution-bounds.md](../architecture/execution-bounds.md#budgets-across-process-boundaries)). |
| Host work | `max_host_work`; CPU-bound native work for builtins and process-boundary serialization/deserialization, in opcode-equivalents (see [execution-bounds.md](../architecture/execution-bounds.md#the-host-work-unit) and [Calibration](#calibration)). |
| I/O bytes | `max_io_bytes`; stdin/stdout, file, HTTP, exec, async completion, and similar byte payload accounting. Per-call reads are also bounded by `max_http_response_bytes`, `max_file_read_bytes`, and `max_exec_output_bytes` (see [execution-bounds.md](../architecture/execution-bounds.md#per-operation-io-limits)). |
| Heap / memory | `max_memory_bytes`, `max_heap`, allocation preflight helpers, and normal heap allocation failures. |
| Host resources | Opaque native Rust resources behind `HeapObject::HostHandle`, such as AWS SDK config/client handles. `max_host_resources` caps total live handles; `limits.host_resources` / `VMBuilder::host_resource_limits` cap live handles per kind. Both caps count the whole process tree. Current and peak counts are tracked per heap. |

## Domain matrix

| Domain | Builtins / paths | Opcode fuel | Host work | I/O bytes | Heap / memory | Host resources | Tests / notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Stdio | `print`, `println`, `read_line`, `read_stdin` | ✅ | N/A | ✅ stdout/stdin bytes charged | ◐ output formatting allocates ordinary strings before byte charge in some display paths | N/A | `print`/`println` charge displayed bytes; `read_stdin` charges returned bytes. |
| Conversion | `int_to_string`, `float_to_string`, `string_to_int`, `char_to_int`, `int_to_char`, `int_to_float`, `word_to_int`, `int_to_word`, `word_to_string`, `string_to_word` | ✅ | ◐ not explicitly charged; operations are small except parsing/formatting | N/A | ✅ returned strings allocated through heap | N/A | Consider explicit low fixed host-work charge only if future audit requires total builtin uniformity. |
| String | `string_length`, `substring`, `string_contains`, `trim`, `split`, `string_replace`, `string_join`, `starts_with`, `ends_with`, `to_upper`, `to_lower` | ✅ | ✅ calibrated: fixed + per KiB walked; `split` and `string_join` also per part | N/A | ✅ preflight for `split`, `replace`, `join`; allocation failures are controlled | N/A | Focused `host_work` tests cover enforcement. Unicode operations remain non-preemptive inside Rust string traversal. |
| Math | `sqrt`, `abs_int`, `abs_float`, `floor`, `ceil` | ✅ | N/A | N/A | N/A | N/A | Scalar, bounded native operations. |
| Width-specific numeric internals | `numeric_int32_*`, `numeric_word32_*`, `numeric_float32_*` | ✅ | N/A | N/A | ◐ checked variants may allocate tuple/option-like results through heap | N/A | Scalar, bounded native operations; wrappers expose these via stdlib modules. |
| Filesystem metadata/mutation | `file_exists`, `remove_file`, `create_dir`, `is_dir`, `is_file` | ✅ | ◐ no explicit host-work charge; policy/path checks and filesystem calls are bounded by OS operation | N/A for metadata; write payload charged separately | ◐ string/path allocations ordinary heap | N/A | Capability policy gates folders; OS calls are synchronous and non-preemptive. |
| Filesystem read/write/listing | `read_file`, `read_file_bytes`, `write_file`, `list_dir`, `glob`, `walk_dir`, `read_file_tagged`, `edit_file_tagged` | ✅ | ◐ no explicit host-work charge for directory traversal/hashline parsing; expensive payload bytes are bounded | ✅ reads/writes/listed path bytes charged; sync and async reads use bounded readers before result allocation | ✅ heap allocations and preflights where needed | N/A | `read_file`, `read_file_bytes`, tagged reads, and edit input reads stop at the smaller of `max_file_read_bytes` and the remaining `max_io_bytes` before allocating results. `glob`/`walk_dir` charge returned path bytes but traversal itself is not host-work metered. |
| Path | `path_join` | ✅ | N/A | N/A | ✅ returned string allocated through heap | N/A | Small deterministic operation. |
| Hash | `blake3` | ✅ | ✅ calibrated: fixed + per KiB | N/A | ✅ digest string allocation through heap | N/A | Focused `host_work` tests cover enforcement. |
| HTTP sync | `http_get`, `http`, `http_json`, `http_msgpack`, `http_bytes` when run synchronously | ✅ | ◐ JSON/msgpack parsing work indirectly covered by heap/memory and byte limits, not by separate HTTP host-work charge | ✅ request body and bounded response bytes charged | ✅ response values allocated through heap; JSON conversion uses allocation checks | N/A | Sync response readers stop at the smaller of `max_http_response_bytes` and the remaining `max_io_bytes` before result allocation. `http_timeout_ms` bounds the whole call, so the non-preemptive host call has a wall-clock ceiling. |
| HTTP async runtime path | `http_get`, `http`, `http_json`, `http_msgpack`, `http_bytes` when `async_io` is active | ✅ for requesting process until suspension | ◐ worker parse/decode not separately host-work charged to worker thread | ✅ request carries the per-call byte limit and HTTP timeouts; completion bytes charged on delivery | ✅ deserialization/allocation into VM heap controlled | N/A | Process suspends while worker runs. Oversized responses fail in worker before delivery. |
| JSON | `json_parse`, `json_to_string`, `json_get`, `json_keys`, `json_length` | ✅ | ✅ calibrated: fixed + per KiB of JSON text (input, or output for `json_to_string`) | N/A | ✅ JSON value conversion has vector/string allocation checks | N/A | Focused `host_work` tests cover enforcement. Costs are intentionally approximate. |
| Bytes | `bytes_length`, `bytes_to_string`, `string_to_bytes`, `bytes_get`, `bytes_slice` | ✅ | ✅ calibrated: fixed + per KiB for conversion/slice paths | N/A | ✅ returned byte/string allocations through heap | N/A | Focused `host_work` tests cover enforcement. |
| Random/RNG | `random_bytes`, `rng_seed`, `rng_bytes`, `rng_int` | ✅ | ✅ calibrated: fixed + per KiB for byte generation/seed input | N/A | ✅ bytes/RNG/tuple allocations through heap | N/A | Focused `host_work` tests cover enforcement. |
| Regex | `regex_match`, `regex_replace` | ✅ | ✅ calibrated: fixed (pattern compile) + per KiB of input | N/A | ✅ `regex_replace` output allocation preflight | N/A | Focused `host_work` tests cover enforcement. Regex compilation/matching is non-preemptive. |
| Environment | `getenv` | ✅ | ◐ no explicit host-work charge; small OS/env lookup | N/A | ✅ optional/string result allocated through heap | N/A | Policy-gated by builtin availability. |
| Time/date | `epoch`, `epoch_ms`, `monotonic_ms`, date/time helpers | ✅ | ◐ no explicit host-work charge; mostly scalar/timezone/library calls | N/A | ✅ returned strings/data allocated through heap where applicable | N/A | Timezone/date formatting can allocate; currently controlled by heap/memory, not `max_host_work`. |
| System sleep | `sleep` | ✅ until suspension/request | N/A | N/A | N/A | N/A | In runtime-managed async mode sleep suspends via `IoRequest::Sleep`; direct blocking behavior is controlled by builtin implementation/runtime path. |
| Exec | `exec` | ✅ | ◐ process spawn/wait not separately host-work charged | ✅ each of stdout/stderr bounded by `max_exec_output_bytes` and the remaining `max_io_bytes`; output charged on success | ✅ result tuple/strings allocated through heap | N/A | Revalidates executable identity before spawn. `exec_timeout_ms` bounds the call. The child leads its own process group, which is killed on timeout or when a stream exceeds its limit. |
| Process/concurrency | `spawn`, `await_process`, `await_process_result`, `cancel`, `wait_any` | ✅ `max_live_processes` / `max_spawns` bound spawns | ✅ spawn capture serialization and child result serialization/deserialization charge process-boundary host work | N/A | ✅ captures/results allocate into child/parent heap under heap limits | N/A | Await/wait operations suspend; sendable payload size drives host-work charge. |
| AWS config | `aws_config_sso_profile`, `aws_config_instance_profile` | ✅ until async request/suspension | ◐ SDK provider-chain work not separately host-work charged | ◐ async completion estimates bytes | ✅ small heap handle allocated | ✅ total/per-kind current and peak handle counts; total/per-kind live limits | Capability-gated by allowed SSO profiles / instance-profile policy. Per-kind config key: `aws_config`. |
| AWS S3 | `aws_s3_client`, `aws_s3_list_buckets` | ✅ | ◐ SDK client construction/listing not separately host-work charged | ◐ async completion estimates bytes | ✅ small heap handle / result values allocated | ✅ total/per-kind current and peak handle counts; total/per-kind live limits | Host handle table stores native client. Per-kind config key: `aws_s3_client`. |
| AWS SQS | `aws_sqs_client`, `aws_sqs_list_queues` | ✅ | ◐ SDK client construction/listing not separately host-work charged | ◐ async completion estimates bytes | ✅ small heap handle / result values allocated | ✅ total/per-kind current and peak handle counts; total/per-kind live limits | Host handle table stores native client. Per-kind config key: `aws_sqs_client`. |
| GitHub issue API | `github_issue_create`, `github_issue_view`, `github_issue_update` and stdlib `Gh.Issue` wrappers | ✅ | ◐ HTTP/JSON work mostly covered by byte/heap limits, not distinct host-work charge | ✅ request/response payloads use HTTP byte accounting where implemented by builtin path | ✅ JSON/string allocations controlled by heap/memory | N/A | Policy-gated by allowed repositories. Verify if new GitHub builtins bypass shared HTTP bounded-reader helpers when extending this domain. |
| Testing | `panic`, `assert`, `assert_eq` | ✅ | ◐ `assert_eq` structural equality can walk large values; no separate host-work charge | N/A | ◐ error messages allocate ordinary strings | N/A | Equality implementation is iterative to avoid host stack overflow; large equality checks remain non-preemptive. |
| Builtin display/output bridge | `print`/`println` display of arbitrary values | ✅ | ◐ display traversal/formatting not separately host-work charged | ✅ displayed bytes charged | ✅ formatted strings allocate outside/inside ordinary Rust before sink write | N/A | For latency-sensitive embedders, avoid printing extremely large structured values or set tight output/heap limits. |
| Process-boundary helpers | `SendableValue` serialization/deserialization for captures/results | N/A (called from runtime/builtin paths) | ✅ fixed + per-KiB payload charges | N/A | ✅ size bounded by `max_message_bytes` before copying; deserialize allocates through receiving heap limits | N/A / ⚠️ host handles are non-sendable except supported AWS config sendable variants | Shape copied iteratively, depth limited; immutable leaves shared where possible. |

## Calibration

`cargo run --release -p hiko-vm --example host_work_calibration [filter]`
measures what each charged builtin costs and what it charges, at inputs of
16 B, 1 KiB, 64 KiB and 1 MiB. For each builtin and size it runs a generated
program that calls the builtin K times and the same program with K = 0, and
subtracts the time of the loop's own opcodes. The rest is the builtin's
cost, expressed in opcode-equivalents: its time divided by the measured time
of one opcode.

### Before tuning (2026-11-01)

Apple M1 Max, release build; one opcode took 12.3 ns. Costs and charges are
in opcode-equivalents; per-KiB values are the slope between 16 B and 1 MiB.
"One call at 1 MiB" is the time of one call at the largest size: a call
cannot be interrupted, so it is the longest pause one call causes.

| Builtin | Cost at 16 B | Cost per KiB | Charged at 16 B | Charged per KiB | Cost / charge at 1 MiB | One call at 1 MiB |
|---|---|---|---|---|---|---|
| `string_length` | 0 | 4.6 | 5 | 0.0 | 947x | 58.7µs |
| `substring` | 11 | 35.7 | 20 | 0.0 | 1828x | 451.3µs |
| `string_contains` | 12 | 4.0 | 10 | 0.0 | 410x | 51.0µs |
| `starts_with` | 7 | 0.0 | 5 | 0.0 | 2x | 429.0ns |
| `ends_with` | 7 | 0.0 | 5 | 0.0 | 2x | 455.0ns |
| `trim` | 3 | 8.2 | 20 | 0.0 | 420x | 104.0µs |
| `to_upper` | 5 | 15.0 | 20 | 0.0 | 770x | 190.4µs |
| `to_lower` | 5 | 13.7 | 20 | 0.0 | 702x | 173.6µs |
| `split` | 24 | 518.5 | 21 | 1.0 | 509x | 6.6ms |
| `string_replace` | 19 | 81.5 | 21 | 1.0 | 80x | 1.0ms |
| `string_join` | 13 | 109.7 | 21 | 1.1 | 101x | 1.4ms |
| `string_to_bytes` | 2 | 7.8 | 11 | 1.0 | 8x | 98.7µs |
| `bytes_to_string` | 7 | 42.0 | 11 | 1.0 | 42x | 531.1µs |
| `bytes_length` | 0 | 0.0 | 5 | 0.0 | 5x | 596.0ns |
| `bytes_get` | 3 | 0.0 | 5 | 0.0 | 2x | 424.0ns |
| `bytes_slice` | 11 | 4.8 | 11 | 0.5 | 9x | 61.0µs |
| `blake3` | 17 | 60.3 | 21 | 1.0 | 59x | 762.8µs |
| `regex_match` | 683 | 1.5 | 50 | 0.0 | 44x | 27.6µs |
| `regex_replace` | 128 | 97.9 | 100 | 0.0 | 1003x | 1.2ms |
| `json_length` | 6 | 135.3 | 50 | 0.0 | 2771x | 1.7ms |
| `json_get` | 33 | 137.5 | 50 | 0.0 | 2817x | 1.7ms |
| `json_parse` | 14 | 268.6 | 100 | 0.0 | 2751x | 3.4ms |
| `json_to_string` | 21 | 339.9 | 100 | 0.0 | 3481x | 4.3ms |
| `random_bytes` | 105 | 420.6 | 11 | 1.0 | 417x | 5.3ms |
| `rng_bytes` | 13 | 56.9 | 11 | 1.0 | 56x | 719.2µs |
| `rng_seed` | 3 | 140.3 | 21 | 1.0 | 138x | 1.8ms |

Most charges are flat or 1 unit per KiB, while real costs grow by 4 to 520
units per KiB, so at 1 MiB most builtins are charged 40x to 3,500x less than
they cost. `starts_with`, `ends_with`, `bytes_length` and `bytes_get` do not
depend on input size and are charged about right.

### After tuning (PR #129)

Each builtin now charges a fixed amount plus a rate per KiB of what it walks
(`split` and `string_join` also per part, `string_replace` also per KiB of
output, the regex builtins also per KiB of pattern). The constants are next
to each builtin in `crates/hiko-vm/src/builtins/`.

The rates are upper bounds: each is set from the most expensive input shape
measured, and the table includes those shapes (`name/shape`): one-character
JSON tokens, a regex with no literal to search for, a long pattern,
replacement output larger than the input, a long prefix, and non-ASCII case
mapping. One opcode took 9.6 ns on this run.

| Builtin (input shape) | Cost / charge at 1 KiB | Cost / charge at 1 MiB | One call at 1 MiB |
|---|---|---|---|
| `string_length` | 5 / 10 | 4479 / 5125 | 43.0µs |
| `substring` | 46 / 57 | 33725 / 36885 | 322.4µs |
| `string_contains` | 12 / 14 | 4130 / 4106 | 39.7µs |
| `string_contains/needle` | 7 / 11 | 40 / 11 | 627.0ns |
| `starts_with` | 8 / 6 | 40 / 6 | 633.0ns |
| `starts_with/long` | 8 / 8 | 3320 / 3077 | 32.0µs |
| `ends_with` | 8 / 6 | 26 / 6 | 494.0ns |
| `trim` | 9 / 14 | 9228 / 9221 | 88.4µs |
| `to_upper` | 16 / 21 | 15593 / 16389 | 149.2µs |
| `to_upper/unicode` | 928 / 965 | 967638 / 983045 | 9.2ms |
| `to_lower` | 16 / 21 | 15825 / 16389 | 151.4µs |
| `to_lower/unicode` | 532 / 555 | 548354 / 563205 | 5.2ms |
| `split` | 439 / 544 | 481376 / 528412 | 4.6ms |
| `string_replace` | 98 / 124 | 84428 / 106516 | 806.7µs |
| `string_replace/grow` | 221 / 184 | 209499 / 167956 | 2.0ms |
| `string_join` | 104 / 157 | 109170 / 139796 | 1.0ms |
| `string_to_bytes` | 9 / 13 | 9356 / 8197 | 89.6µs |
| `bytes_to_string` | 49 / 52 | 45052 / 43018 | 430.6µs |
| `bytes_length` | 0 / 5 | 0 / 5 | 226.0ns |
| `bytes_get` | 4 / 5 | 9 / 5 | 332.0ns |
| `bytes_slice` | 15 / 15 | 4409 / 5130 | 42.4µs |
| `blake3` | 133 / 82 | 59762 / 63508 | 571.1µs |
| `regex_match` | 699 / 1525 | 2078 / 3571 | 20.1µs |
| `regex_match/class` | 2740 / 1581 | 4199 / 3627 | 40.4µs |
| `regex_match/pattern` | 16430 / 13001 | fails (over the regex size limit) | - |
| `regex_replace` | 234 / 1642 | 99290 / 134632 | 948.7µs |
| `regex_replace/grow` | 248 / 1642 | 118689 / 134632 | 1.1ms |
| `json_length` | 579 / 826 | 758212 / 798766 | 7.2ms |
| `json_get` | 643 / 831 | 755214 / 798771 | 7.2ms |
| `json_parse` | 1798 / 2228 | 2230301 / 2232368 | 21.3ms |
| `json_to_string` | 1938 / 2468 | 2602148 / 2467898 | 24.9ms |
| `random_bytes` | 421 / 526 | 431710 / 431209 | 4.1ms |
| `rng_bytes` | 70 / 72 | 60260 / 58383 | 575.9µs |
| `rng_seed` | 138 / 145 | 141616 / 143365 | 1.4ms |
| `rng_int` | 7 / 5 | 7 / 5 | 310.0ns |
| `json_keys` | 600 / 831 | 828257 / 798771 | 7.9ms |

- Most charges are within 1.3x of the measured cost.
- **Cheaper shapes are overcharged.** JSON with longer tokens costs up to
  about 4x less per KiB than one-character tokens. A regex with a literal
  prefix compiles in about 700 units against a fixed charge of 1,500, so
  short regex calls are charged 2x to 7x their cost.
- **Under the bound by up to 1.7x:** a character-class regex on short input
  (2,740 against 1,581), and `string_replace` with output much larger than
  its input. A pattern over the regex crate's compiled-size limit (about
  10 MiB compiled) fails instead of compiling, which bounds compile time.
- **Constant-cost builtins** (`starts_with` with a short prefix, `ends_with`,
  `bytes_get`, `rng_int`) differ by under 1 µs, which is measurement noise.
- **Charged after the work:** `json_to_string` charges per KiB of the text it
  produced, and `substring` per KiB up to the end of the result, because
  those sizes are known only afterwards. Both charge their fixed part first,
  so a spent budget stops them before they start.

## Known follow-ups

1. **Calibration.** Rerun `host_work_calibration` after changing a builtin and
   update its rates; the tables above are for one machine.
2. **Traversal-only host work.** Some directory traversal, display formatting,
   structural equality, date/time formatting, conversion parsing, and provider
   SDK work is controlled by broader limits but not individually charged with
   fine-grained host-work units.

## Audit checklist for new builtins

When adding a builtin, update this matrix and answer:

- Does one call do CPU work disproportionate to one opcode? If yes, charge
  `max_host_work`.
- Can it read/write/fetch/print bytes? If yes, charge and/or pre-bound
  `max_io_bytes` before allocating large results.
- Can it allocate host-side intermediates? If yes, preflight heap/memory where
  practical.
- Does it create or retain native Rust resources? If yes, use host-resource
  accounting and document its per-kind limit key.
- Is it synchronous and potentially slow? Document that it is non-preemptive or
  route it through runtime-managed async I/O.
- Are failure modes controlled and deterministic when limits are exhausted?
