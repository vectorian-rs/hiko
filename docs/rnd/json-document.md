# Native JSON documents: design foundation

**Status: proposal, not implemented.** Tracked by [#256](https://github.com/vectorian-rs/hiko/issues/256)
under [#205](https://github.com/vectorian-rs/hiko/issues/205). This document is a
reviewable foundation, not approval to change the VM, public API or budgets.
The name `Std.JsonDoc` and every signature/policy key below are proposed.

Goal: parse once, access indexed native data, and materialize only requested
Hiko values. Do not silently cache text operations or replace `Std.Json.json`.
Streaming/selective parsing, mutation, JSONPath and cross-process transport
are outside the first implementation. Unlike the historical
[growth-audit cache suggestion](growth-audit.md), this proposal makes ownership explicit.

## Current behavior and motivation

Source inspected: main `947ccc3d9cbd7a7cd9315389900330c96764b36f`.
[Text access](../../crates/hiko-vm/src/builtins/json.rs) reparses the complete
input on each call. [Typed conversion](../../crates/hiko-vm/src/builtins/json_value.rs)
builds Hiko constructor nodes, field tuples and linked lists for the entire
value. `JObject` field and `JArray` index traversal are linear in their lists.
The [existing JSON API](../builtins/builtins.md#json) remains the current contract.

The #256 investigation used a two-field object containing an n-element array
and `answer=42`, then looked up `answer` n times. Complete untimed stack runs:

| n | Text VM / host work | Typed VM / host work | VM peak bytes: text / typed |
|---:|---:|---:|---:|
| 512 | 11838 / 431616 | 27227 / 2274 | 59067 / 146720 |
| 1024 | 23614 / 1661952 | 54363 / 4454 | 61339 / 291104 |
| 2048 | 47166 / 6518784 | 108635 / 8814 | 63387 / 579872 |

VM::new, fuel/host guards each 1e9, default heap/GC, hermetic imports, setup
and validation included; no subtraction. These are NOT native-document results
or timings. Peaks are absolute estimated VM lifetime peaks, including garbage,
not total process memory or post-GC reachable storage. Temporary serde storage
is not fully included. See [investigation evidence](https://github.com/vectorian-rs/hiko/issues/256#issuecomment-6958457089).
No production text-access hot loop was found; do not invent a deployed speedup.
A real caller needing only `answer` should extract it once, not repeat lookup.

## Recommended decisions

| Concern | Recommendation for review |
|---|---|
| Representation | Opaque immutable document owner and lightweight value views. |
| Arrays | Contiguous child-node indices; direct indexed access. |
| Objects | Sorted index of decoded keys; binary search and deterministic iteration. |
| Numbers | Preserve validated number tokens; never implicitly narrow to f64. |
| Compatibility | New opt-in API; existing text/ADT semantics and encodings unchanged. |
| Accounting | One owner charge plus views, with incremental scratch reservations. |

A sorted index gives O(log m) key comparisons for m fields, not O(1) or O(n).
Charge compared bytes as well as index steps; long common prefixes still cost.
This avoids making an adversarial hash table part of the first bounded API.
A hash index is a later measured choice, not a second simultaneous lookup path.

## Proposed surface and semantics

One protected opaque type `value` represents any root or subtree. Source cannot
construct owners/views, expose node IDs, or cast another host handle to it.
Reuse [builtin ownership metadata](../../crates/hiko-builtin-meta/src/internal.rs)
and existing protected-type checks; do not add a parallel ABI/name registry.
`kind` is a small public classification: Null, Boolean, Number, Text, Array, Object.
Signatures below are schematic, not executable declarations or a frozen ABI.

| Operation in JsonDoc | Proposed signature | Meaning |
|---|---|---|
| parse | `string -> value` | Validate the whole input, then publish its root. |
| kind | `value -> kind` | Inspect without building a Hiko tree. |
| field | `string -> value -> value Option.option` | Exact decoded object key. |
| item | `int -> value -> value Option.option` | Zero-based array position. |
| length | `value -> int Option.option` | Array/object size only, not string length. |
| entry | `int -> value -> (string * value) Option.option` | Object entry in key order. |
| as_bool | `value -> bool Option.option` | Copy a boolean scalar. |
| as_int | `value -> int Option.option` | Checked signed-64 integer-token conversion. |
| as_float | `value -> float Option.option` | Explicit approximate, finite conversion. |
| as_string | `value -> string Option.option` | Materialize decoded string contents. |
| number_text | `value -> string Option.option` | Materialize exact numeric token. |
| to_string | `value -> string` | Serialize selected value, preserving number tokens. |
| detach | `value -> value` | Copy selected subtree into an independent owner. |

Missing keys, wrong container kinds and invalid/negative positions return
Option.None. JSON null is Option.Some of a view whose kind is Null; an empty
JSON string is a Text view, not absence. Scalar extractors return None for
wrong kinds; numeric conversion errors are distinct from absence (see below).

Fields are literal strings: `"0"`, `"a.b"` and `""` are ordinary object keys.
There is no dotted path parsing, Unicode normalization or string-to-index coercion.
Array indexing uses item, not field. Object keys compare decoded Unicode text
lexicographically, matching current parsed-object ordering; equivalent escaped
spellings identify the same key. Duplicate keys are last-wins. Parse still
validates every duplicate value, including those discarded by last-wins.
Entry iteration follows the sorted surviving keys; array iteration uses item.
No eager Hiko key/element list is necessary. Item/length are O(1); entry adds
key materialization cost. String extraction and writing cost their output size.

Parse must reject malformed suffixes even if a requested field precedes them.
Adopt an explicit proposed maximum container depth of 128 (root container is
level 1); test 128/129. This is a new-API decision, not a change to old limits.
Do not silently accept comments, NaN, Infinity, trailing commas or invalid escapes.

## Native layout and GC ownership

Use a flat arena: scalar nodes, contiguous child-index ranges, object member
indices and bounded byte storage. Validate source spans, node IDs, offsets and
checked-size arithmetic before publication. Do not wrap a retained serde tree
in the existing [HostHandle](../../crates/hiko-vm/src/value.rs) as a shortcut:
its native payload and GC/accounting behavior are not this contract.

An owner holds boxed arena storage and traces its immutable input Hiko string
through a GcRef. That existing string remains byte-accounted while any document
needs its number/string spans. Arena nodes contain internal indices, not per-node
GC objects. Escaped keys are decoded/indexed during parsing; ordinary value
strings can stay validated source spans until extraction. Count native decoded
keys and all index capacities, including storage for superseded duplicates.

Views contain a GC edge to the owner plus a validated node ID, not raw pointers
or a chain of parent views. The collector traces owner/input edges and accounts
views normally. Last owner/view death releases the native allocation once;
failure/cancellation releases unpublished scratch. No global registry/cache
may keep an otherwise unreachable document alive. No user-visible close call.

A tiny view retains the WHOLE owner and input. This is an explicit tradeoff,
not a memory optimization claim. as_string copies its result into a regular
Hiko string; bool/int/float extraction retains no document reference. detach
must create an owner with no edge/buffer reference to the original, preserving
selected-value semantics and exact number tokens. Its O(subtree size) copying
and simultaneous old/new storage are charged; it is not a free lifetime trick.

Keep large native structs boxed so a new enum variant does not inflate every
existing HeapObject. Preserve Value/GcRef and existing JSON constructor encodings;
measure/check layouts rather than assuming an opaque pointer makes them unchanged.
Audit every exhaustive heap visitor, estimator, formatter and boundary codec.
Diagnostic display stays opaque; no implicit unmetered document serialization.
Do not add implicit structural equality/hashing. Opaque equality behavior must be
settled in the implementation plan, not inherited accidentally from GC identity.

## Memory and work contract: implementation gates

Reuse the [execution-bound contract](../architecture/execution-bounds.md) and
[accounting matrix](../builtins/accounting-matrix.md); do not loosen their pins.
The memory limit remains an explicit estimate of admitted capacities, not RSS.
Allocator metadata/rounding outside that model must be documented, not called
an exact physical bound. Native payloads must appear in usage/peak counters.

1. Before any parser/sorter/decoder allocation or capacity growth, reserve its
   checked charge against the SAME applicable memory budget as Hiko storage.
   Include temporary frames, token/count buffers, key decoding, index building,
   sorting scratch, output buffers and old+new capacities during reallocation.
2. Allocation must use fallible bounded growth under that reservation. An
   allocate-first/estimate-after serde_json::Value parse is not acceptable.
   Use a budget-aware parser/visitor or bounded counting/build passes; select
   the concrete algorithm and reservation proof in the implementation plan.
3. Commit native retained capacities to the owner ledger once; release scratch
   reservations on every error. Views charge only their own wrapper storage,
   not the owner's bytes again. Do not introduce a second usage counter that
   can disagree with admission/GC. Shared input is traced and counted once in
   its owning heap; do not change existing cross-heap Arc accounting globally.
4. max_heap still counts actual GC objects, NOT fictional per-native-node Hiko
   objects. Native nodes/edges are bounded by byte/work admission and checked
   index ranges. Owner and view allocations obey existing object limits.
5. Pay fixed entry work before native processing. Meter parse bytes/tokens,
   key comparisons/sorting, binary-search steps, extraction/escaping/copying
   and iteration. Incrementally reject exhausted host budgets before more work;
   do not bill only after an expensive parse, comparison or output is complete.
6. Runtime failure publishes no root, view or partial output to the caller.
   It need not refund work already consumed or roll back unrelated VM effects.
   GC pressure cannot collect an active input/owner, and retries must not repeat
   a partially charged/effectful native operation. Test roots under forced GC.

Metered does not mean preemptible. Synchronous parsing/GC still has latency
limits; budget checks must bound each native chunk. Do not promise run_slice
fairness inside one native call. Any resumable parser is a separate design.

## Number fidelity and serialization

Keep validated original number tokens, including huge integers, exponent forms,
1.0 and negative zero. Parsing a syntactically valid number must not require it
to fit i64/f64. number_text and to_string preserve those tokens exactly.
Writing may normalize whitespace/string escapes and orders keys lexicographically;
it is not an original-document byte round trip. Retain no discarded duplicate keys
in serialized output, even if arena capacity still includes their parsed nodes.

as_int accepts integer-form tokens in signed-64 range; fraction/exponent forms
or overflow raise a conversion diagnostic, not rounded/truncated output or None.
as_float explicitly opts into f64 rounding; reject non-finite/out-of-range results.
Define underflow as a conversion error when a nonzero token would become zero.
For example 9223372036854775809 stays exact until the caller requests as_float;
1.0 stays 1.0 in serialization but is not accepted by integer-token extraction.
No implicit conversion to the existing Json.json ADT: its JInt/JFloat constraints
cannot represent every such number losslessly. A future adapter must specify
checked failures rather than silently recreating the current precision loss.

## Authority, diagnostics and process boundaries

Recommend using the existing builtin-json build family, but a NEW default-off
capabilities.json_doc.<operation> leaf for each native operation above. These
keys do not exist today. Metadata/config/audit generation must agree, with no
implicit grants from old json.* leaves or from importing a module. Reject
unused peer-family requirements in a JSON-only build; availability is not permission.

First release: owners AND views are non-sendable, including nested captures,
arguments and results. Both runtimes reject them at process-boundary validation
before publication; no pointer transport or automatic stringify/detach fallback.
An explicitly extracted string/int is ordinary sendable data. Later transport
requires its own accounting, authority and sharing/copying decision.

Diagnostics name the proposed operation and cause: invalid JSON with input byte
offset, depth excess, conversion failure, resource refusal, or unsupported process
transfer. Preserve the Hiko call-site span where available. Position reporting,
error formatting and cleanup must themselves be bounded; do not rescan an entire
input or dump sensitive JSON contents merely to produce an error. Existing text
and ADT errors remain unchanged. No new broad catch-and-return-empty behavior.

## Integration map (future code, not changed by this PR)

- [Builtin metadata](../../crates/hiko-builtin-meta/src/internal.rs) and signatures:
  protected opaque type, trusted forwarding, one authoritative operation registry.
- [Value/heap](../../crates/hiko-vm/src/value.rs) and
  [heap admission](../../crates/hiko-vm/src/heap.rs): boxed owner, views, reservations,
  capacity estimates and release; [GC](../../crates/hiko-vm/src/vm/gc.rs): roots/edges.
- [Config](../../crates/hiko-vm/src/config.rs) and builtin audit: default-off leaf mapping;
  future Std module and generated pins/docs only after the API is approved.
- [Boundary codec](../../crates/hiko-vm/src/sendable.rs) and
  [shape validation](../../crates/hiko-vm/src/shape_check.rs): reject document transfer.

## Approval-gated delivery

This PR contains design documentation only, no parser scaffold, API registration,
new capability keys, runtime tags, dependencies or claimed native performance.
Before M1, approve an implementation issue naming the exact API, parser algorithm,
reservation ledger, layouts, limits and tests. Design-document merge is not that gate.

| Milestone | Deliverable and exit evidence |
|---|---|
| M1 | Owner/type/GC plus bounded whole-input parser; root/publication/cleanup and limits tests. |
| M2 | Sorted field and indexed-array access, scalar extraction and iteration; budget/parity tests. |
| M3 | Bounded writer and detach; exact-number and independent-lifetime tests. |
| M4 | Calibrated charges and frozen comparative workloads; explicit rollout decision. |

Each implementation section needs default/all and minimal JSON builds, stack
and register-enabled behavior, both runtimes' boundary controls, unchanged legacy
JSON tests and required reviews/CI. No placeholder PR per milestone: combine
coherent approved work where practical, and never expose incomplete public operations.

### Required observable cases

- Literal numeric/dotted/empty/Unicode keys; escaped-equivalent duplicates are
  last-wins; absent vs Null vs Text "" remain distinct; negative/out-of-range indices.
- Empty/scalar roots; deep128 succeeds/deep129 refuses; malformed suffix, escapes
  and every discarded duplicate still validated before publishing a root.
- Integer min/max, uint above i64, huge exponents, 1.0, -0, underflow and Unicode:
  exact token output; explicit conversion success/error; no silent numeric corruption.
- A surviving child view keeps its owner valid across GC; dropping all views frees
  owner/input; detach survives original collection without retaining its buffers.
- Fail each reservation/growth/host-work/GC boundary; no partial result or leaked
  reservation, valid roots preserved, no quota/pin relaxation to obtain success.
- Old grants deny new operations and vice versa; per-operation grants stay narrow;
  forged/wrong-kind handles and process transfers fail before publication.

### Comparative evidence before claiming efficiency

Freeze sources/revisions and retain all refusals/failures. Compare text access,
existing typed tree and native document on one/few fields, repeated same field,
wide first/last/missing fields, arrays, long keys/strings and nested input.
Include parse/build/index cost, setup and validation; report opcode AND host work,
actual native allocation capacity/peak alongside VM estimates and retained storage.
One lookup may still favor text access; no universal speed/memory win is assumed.
Deliberately test repeated last-field scans to distinguish indexing from parse reuse.
Only run wall-clock/RSS experiments under a fresh approved bounded protocol;
old #256 counters do not calibrate native charges or establish timing acceptance.

## Review decisions still required

Approve/adjust the surface, sorted-index choice, exact-token number semantics,
128-depth definition, narrow capability leaves and non-sendable first release.
The concrete parser/sorter, native reservation integration and ABI audit are
implementation-plan blockers, not details that may be skipped after this PR.
