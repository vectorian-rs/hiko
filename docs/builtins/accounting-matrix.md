# Builtin Resource Accounting Matrix

This document audits builtin and builtin-adjacent resource accounting as of the
first-pass #63 implementation and the #80 host-resource work in progress.

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
| Opcode fuel | VM dispatch fuel / `max_work` / `max_fuel`; every executed opcode consumes fuel. Builtin calls consume opcode fuel for the call dispatch, but native work inside the builtin needs separate accounting. |
| Host work | `max_host_work`; approximate CPU-bound native work units for builtins and process-boundary serialization/deserialization. |
| I/O bytes | `max_io_bytes`; stdin/stdout, file, HTTP, exec, async completion, and similar byte payload accounting. |
| Heap / memory | `max_memory_bytes`, `max_heap`, allocation preflight helpers, and normal heap allocation failures. |
| Host resources | Opaque native Rust resources behind `HeapObject::HostHandle`, such as AWS SDK config/client handles. Full host-resource limits are tracked by #80. |

## Domain matrix

| Domain | Builtins / paths | Opcode fuel | Host work | I/O bytes | Heap / memory | Host resources | Tests / notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Stdio | `print`, `println`, `read_line`, `read_stdin` | ✅ | N/A | ✅ stdout/stdin bytes charged | ◐ output formatting allocates ordinary strings before byte charge in some display paths | N/A | `print`/`println` charge displayed bytes; `read_stdin` charges returned bytes. |
| Conversion | `int_to_string`, `float_to_string`, `string_to_int`, `char_to_int`, `int_to_char`, `int_to_float`, `word_to_int`, `int_to_word`, `word_to_string`, `string_to_word` | ✅ | ◐ not explicitly charged; operations are small except parsing/formatting | N/A | ✅ returned strings allocated through heap | N/A | Consider explicit low fixed host-work charge only if future audit requires total builtin uniformity. |
| String | `string_length`, `substring`, `string_contains`, `trim`, `split`, `string_replace`, `string_join`, `starts_with`, `ends_with`, `to_upper`, `to_lower` | ✅ | ✅ fixed + per-KiB first-pass charges for cheap ops, scans, and allocation-heavy paths | N/A | ✅ preflight for `split`, `replace`, `join`; allocation failures are controlled | N/A | Focused `host_work` tests cover enforcement. Unicode operations remain non-preemptive inside Rust string traversal. |
| Math | `sqrt`, `abs_int`, `abs_float`, `floor`, `ceil` | ✅ | N/A | N/A | N/A | N/A | Scalar, bounded native operations. |
| Width-specific numeric internals | `numeric_int32_*`, `numeric_word32_*`, `numeric_float32_*` | ✅ | N/A | N/A | ◐ checked variants may allocate tuple/option-like results through heap | N/A | Scalar, bounded native operations; wrappers expose these via stdlib modules. |
| Filesystem metadata/mutation | `file_exists`, `remove_file`, `create_dir`, `is_dir`, `is_file` | ✅ | ◐ no explicit host-work charge; policy/path checks and filesystem calls are bounded by OS operation | N/A for metadata; write payload charged separately | ◐ string/path allocations ordinary heap | N/A | Capability policy gates folders; OS calls are synchronous and non-preemptive. |
| Filesystem read/write/listing | `read_file`, `read_file_bytes`, `write_file`, `list_dir`, `glob`, `walk_dir`, `read_file_tagged`, `edit_file_tagged` | ✅ | ◐ no explicit host-work charge for directory traversal/hashline parsing; expensive payload bytes are bounded | ✅ reads/writes/listed path bytes charged; sync and async reads use bounded readers before result allocation | ✅ heap allocations and preflights where needed | N/A | `read_file`, `read_file_bytes`, tagged reads, and edit input reads use `max_io_bytes` before allocating results. `glob`/`walk_dir` charge returned path bytes but traversal itself is not host-work metered. |
| Path | `path_join` | ✅ | N/A | N/A | ✅ returned string allocated through heap | N/A | Small deterministic operation. |
| Hash | `blake3` | ✅ | ✅ fixed + per-KiB charge | N/A | ✅ digest string allocation through heap | N/A | Focused `host_work` tests cover enforcement. |
| HTTP sync | `http_get`, `http`, `http_json`, `http_msgpack`, `http_bytes` when run synchronously | ✅ | ◐ JSON/msgpack parsing work indirectly covered by heap/memory and byte limits, not by separate HTTP host-work charge | ✅ request body and bounded response bytes charged | ✅ response values allocated through heap; JSON conversion uses allocation checks | N/A | Sync response readers are bounded by remaining `max_io_bytes` before result allocation. Host call is still non-preemptive while inside Rust/ureq. |
| HTTP async runtime path | `http_get`, `http`, `http_json`, `http_msgpack`, `http_bytes` when `async_io` is active | ✅ for requesting process until suspension | ◐ worker parse/decode not separately host-work charged to worker thread | ✅ request carries remaining byte budget; completion bytes charged on delivery | ✅ deserialization/allocation into VM heap controlled | N/A | Process suspends while worker runs. Oversized responses fail in worker before delivery. |
| JSON | `json_parse`, `json_to_string`, `json_get`, `json_keys`, `json_length` | ✅ | ✅ fixed first-pass charges | N/A | ✅ JSON value conversion has vector/string allocation checks | N/A | Focused `host_work` tests cover enforcement. Costs are intentionally approximate. |
| Bytes | `bytes_length`, `bytes_to_string`, `string_to_bytes`, `bytes_get`, `bytes_slice` | ✅ | ✅ fixed + per-KiB charges for conversion/slice paths | N/A | ✅ returned byte/string allocations through heap | N/A | Focused `host_work` tests cover enforcement. |
| Random/RNG | `random_bytes`, `rng_seed`, `rng_bytes`, `rng_int` | ✅ | ✅ fixed + per-KiB charges for byte generation/seed input | N/A | ✅ bytes/RNG/tuple allocations through heap | N/A | Focused `host_work` tests cover enforcement. |
| Regex | `regex_match`, `regex_replace` | ✅ | ✅ fixed first-pass charges | N/A | ✅ `regex_replace` output allocation preflight | N/A | Focused `host_work` tests cover enforcement. Regex compilation/matching is non-preemptive. |
| Environment | `getenv` | ✅ | ◐ no explicit host-work charge; small OS/env lookup | N/A | ✅ optional/string result allocated through heap | N/A | Policy-gated by builtin availability. |
| Time/date | `epoch`, `epoch_ms`, `monotonic_ms`, date/time helpers | ✅ | ◐ no explicit host-work charge; mostly scalar/timezone/library calls | N/A | ✅ returned strings/data allocated through heap where applicable | N/A | Timezone/date formatting can allocate; currently controlled by heap/memory, not `max_host_work`. |
| System sleep | `sleep` | ✅ until suspension/request | N/A | N/A | N/A | N/A | In runtime-managed async mode sleep suspends via `IoRequest::Sleep`; direct blocking behavior is controlled by builtin implementation/runtime path. |
| Exec | `exec` | ✅ | ◐ process spawn/wait not separately host-work charged | ✅ stdout/stderr bounded and charged by `max_io_bytes` | ✅ result tuple/strings allocated through heap | N/A | Revalidates executable identity before spawn; timeout configured by exec policy. Reader threads reject oversized output before returning it to the VM. |
| Process/concurrency | `spawn`, `await_process`, `await_process_result`, `cancel`, `wait_any` | ✅ | ✅ spawn capture serialization and child result serialization/deserialization charge process-boundary host work | N/A | ✅ captures/results allocate into child/parent heap under heap limits | N/A | Await/wait operations suspend; sendable payload size drives host-work charge. |
| AWS config | `aws_config_sso_profile`, `aws_config_instance_profile` | ✅ until async request/suspension | ◐ SDK provider-chain work not separately host-work charged | ◐ async completion estimates bytes | ✅ small heap handle allocated | ⚠️ native `SdkConfig` resource accounting tracked by #80 | Capability-gated by allowed SSO profiles / instance-profile policy. |
| AWS S3 | `aws_s3_client`, `aws_s3_list_buckets` | ✅ | ◐ SDK client construction/listing not separately host-work charged | ◐ async completion estimates bytes | ✅ small heap handle / result values allocated | ⚠️ native S3 client resource accounting tracked by #80 | Host handle table stores native client; full per-kind limits are #80. |
| AWS SQS | `aws_sqs_client`, `aws_sqs_list_queues` | ✅ | ◐ SDK client construction/listing not separately host-work charged | ◐ async completion estimates bytes | ✅ small heap handle / result values allocated | ⚠️ native SQS client resource accounting tracked by #80 | Host handle table stores native client; full per-kind limits are #80. |
| GitHub issue API | `github_issue_create`, `github_issue_view`, `github_issue_update` and stdlib `Gh.Issue` wrappers | ✅ | ◐ HTTP/JSON work mostly covered by byte/heap limits, not distinct host-work charge | ✅ request/response payloads use HTTP byte accounting where implemented by builtin path | ✅ JSON/string allocations controlled by heap/memory | N/A | Policy-gated by allowed repositories. Verify if new GitHub builtins bypass shared HTTP bounded-reader helpers when extending this domain. |
| Testing | `panic`, `assert`, `assert_eq` | ✅ | ◐ `assert_eq` structural equality can walk large values; no separate host-work charge | N/A | ◐ error messages allocate ordinary strings | N/A | Equality implementation is iterative to avoid host stack overflow; large equality checks remain non-preemptive. |
| Builtin display/output bridge | `print`/`println` display of arbitrary values | ✅ | ◐ display traversal/formatting not separately host-work charged | ✅ displayed bytes charged | ✅ formatted strings allocate outside/inside ordinary Rust before sink write | N/A | For latency-sensitive embedders, avoid printing extremely large structured values or set tight output/heap limits. |
| Process-boundary helpers | `SendableValue` serialization/deserialization for captures/results | N/A (called from runtime/builtin paths) | ✅ fixed + per-KiB payload charges | N/A | ✅ deserialize allocates through receiving heap limits | N/A / ⚠️ host handles are non-sendable except supported AWS config sendable variants | Shape copied; immutable leaves shared where possible. |

## Known follow-ups

1. **#80 host-resource limits.** AWS config/client handles need explicit native
   resource count/peak/limit accounting in addition to heap handle accounting.
2. **Calibration.** `max_host_work` constants are deliberately approximate.
   Benchmark representative workloads before treating unit values as release
   guidance.
3. **Per-operation byte knobs.** Global `max_io_bytes` currently bounds HTTP,
   file, exec, stdin, stdout, and async completions. If operators need different
   budgets per operation, add separate knobs such as `max_http_response_bytes`,
   `max_file_read_bytes`, or `max_exec_output_bytes`.
4. **Traversal-only host work.** Some directory traversal, display formatting,
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
  accounting (#80).
- Is it synchronous and potentially slow? Document that it is non-preemptive or
  route it through runtime-managed async I/O.
- Are failure modes controlled and deterministic when limits are exhausted?
