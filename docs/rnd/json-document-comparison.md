# JSON documents: standards and implementation comparison

**Status: research/feasibility, not a runtime parser or accepted API.**
Supports the [native-document foundation](json-document.md) for
[#256](https://github.com/vectorian-rs/hiko/issues/256). The simd-json spike below
adds untimed tests; other comparisons are source inspection. Neither constitutes
independent acceptance, full conformance or performance measurement. Cross-language
versions below are representative implementations, not claims of latest.

## Verdict

The foundation is a legitimate JSON document/DOM architecture proposal, but not
a complete parsing specification. .NET supplies close owner/view precedent;
Go, Rust and .NET support retaining numeric text before explicit conversion.
No standard prescribes the object index, memory layout, GC model or capability API.
Keep those recommendations, but close the input-profile and implementation gates
below before treating the design as an implementation-ready parser contract.
The [simd-json owned-DOM spike](#simd-json-owned-dom-feasibility-264) is **NO-GO
as-is** under the foundation's fidelity/admission requirements; #265 must wait.

## What established implementations do

### Python: ordinary native containers

[CPython 3.14's scanner](https://github.com/python/cpython/blob/v3.14.0/Modules/_json.c)
constructs dicts, lists and Python scalars. Integers become Python int, subject
to interpreter digit/resource limits; fractional/exponent values normally become
float. [Python's API](https://docs.python.org/3/library/json.html) offers parse_int
and parse_float hooks, including Decimal conversion. Containers and scalars have
ordinary GC ownership, not borrowed document views.

This avoids interpreting a field-list traversal in user bytecode, but still
materializes a full object tree. Python's default decoder also accepts
NaN/Infinity extensions and lone surrogate values; it is not a strict-mode
policy to copy blindly.

### JavaScript: runtime object layouts and arrays

[ECMAScript JSON.parse](https://tc39.es/ecma262/multipage/structured-data.html#sec-json.parse)
produces ordinary objects, arrays and scalars. The
[V8 13.8.258 parser](https://github.com/v8/v8/blob/13.8.258/src/json/json-parser.cc)
uses JSDataObjectBuilder and the engine's property/element representations,
not a user-language tagged list tree. Engine layouts are implementation details,
not JSON-specified hash-table guarantees.

Normal numbers become binary64 Number and can lose integer precision. Modern
reviver source context permits explicit recovery from original primitive text;
that is not the default numeric representation. Object enumeration follows
JavaScript's property rules, not a universal promise to preserve JSON source order.

### Go: maps/slices or direct typed decoding

[Go 1.25 encoding/json](https://github.com/golang/go/blob/go1.25.0/src/encoding/json/decode.go)
generic decoding uses map[string]any, []any and scalars; typed decoding can
populate structs directly. Generic numbers default to float64.
[Decoder.UseNumber](https://pkg.go.dev/encoding/json#Decoder.UseNumber) instead
retains the literal as json.Number until Int64/Float64 conversion.

Exact tokens with explicit extraction are established practice. Go replaces
invalid UTF-8/UTF-16 surrogate sequences when decoding strings, unlike a
reject-on-invalid Unicode-scalar profile. Decoder's streaming operation and
whole-input Unmarshal also have different input-consumption contracts.

### Rust: ordered maps, arrays and optional raw values

[serde_json 1.0.145 maps](https://github.com/serde-rs/json/blob/v1.0.145/src/map.rs)
use BTreeMap by default; preserve_order selects IndexMap. Arrays use Vec.
[Number](https://github.com/serde-rs/json/blob/v1.0.145/src/number.rs) normally
supports u64/i64/f64; arbitrary_precision changes numeric storage. Do not infer
an all-token spelling guarantee merely from that feature.
[RawValue](https://github.com/serde-rs/json/blob/v1.0.145/src/raw.rs) borrows or
owns a validated raw JSON range and preserves its original formatting, avoiding
full materialization of that range. It is not itself an indexed document.

Ordered logarithmic lookup therefore has precedent. Hiko's current uint loss
is in conversion to its JInt/JFloat ADT, not proof serde cannot retain uint64.
Neither Value nor RawValue automatically enforces Hiko's allocation/work budgets.

### .NET: closest document/view precedent

[.NET's DOM guidance](https://learn.microsoft.com/en-us/dotnet/standard/serialization/system-text-json/use-dom)
and [the .NET 10 source](https://github.com/dotnet/runtime/blob/v10.0.0/src/libraries/System.Text.Json/src/System/Text/Json/Document/JsonDocument.cs)
show JsonDocument retaining UTF-8 input plus compact metadata. JsonElement
stores a parent reference and metadata index. Raw numeric bytes remain available
for conversion/writing. Disposal controls document lifetime; Clone can copy
selected metadata and raw bytes. Already-cloned data can reuse its owner, so
Hiko's proposed unconditional independent detach is a stronger guarantee.

Importantly, property lookup is sequential. The guidance explicitly prioritizes
initial parse cost over lookup cost and recommends enumeration. Array indexing
is direct for simple children, but complex arrays traverse metadata. A native
DOM does not automatically provide O(1) random access.

Hiko would use GC ownership rather than manual disposal. Its sorted object
index and contiguous child index are extra construction/storage costs, chosen
to improve repeated access—not requirements of this established model.

## Standard requirements versus Hiko choices

Use [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259) and
[ECMA-404](https://ecma-international.org/publications-and-standards/standards/ecma-404/)
as the normative syntax basis rather than inventing another JSON grammar.

- A complete text is one value, including scalar roots, surrounded only by
  JSON whitespace: SP, HT, LF and CR. Unicode whitespace in general is not JSON
  whitespace; adjacent root values are not one complete text.
- String escapes, controls, number forms, delimiters and trailing input need
  explicit coverage. Rejecting comments/NaN/Infinity is the strict grammar choice.
- Duplicate object names SHOULD be unique; RFC8259 does not mandate last-wins.
  Last-wins is widespread and legitimate, but remains an explicit API policy.
- Objects are semantically unordered. Sorted iteration is a Hiko choice, not
  required JSON behavior or canonical-JSON certification.
- Parsers may set resource, depth, string-content and numeric limits. Depth128
  is a Hiko proposal, not a JSON standard limit. Raw number tokens and documented
  representational limits are both established implementation choices.

## Gaps to close before native implementation

### 1. Declare the exact input profile

Add a normative grammar reference and define complete-input success, whitespace,
BOM behavior and error positions. For example, true followed by NBSP is not a
valid whole JSON text; neither is true followed by false. A leading U+FEFF can
exist in a Hiko string even though it is valid UTF-8. RFC8259 permits a parser
to ignore an initial BOM; the Hiko API must explicitly choose reject or ignore.
These are remaining decisions, not behavior implemented by this research PR.

### 2. Settle unpaired UTF-16 escape handling

For the JSON text `"\uDEAD"`, RFC8259 section8.2 explicitly notes that its
grammar can contain unpaired surrogate escapes with interoperability problems.
“Invalid escapes” alone does not define this case. Python/JavaScript can retain
surrogate values in their string models; Go replaces invalid pairs.

For Hiko's UTF-8 scalar string representation, recommend a declared profile
that combines valid pairs and rejects unpaired surrogates during parsing.
That recommendation needs review, not silent adoption. Test names AND values,
including discarded duplicate values. Do not mislabel a syntactically complete
surrogate escape as an unknown escape. Define invalid-byte tests at the UTF-8
input boundary because the proposed public input is already a Hiko string.

### 3. Add a conformance gate, separate from API/resource checks

The old 29 API checks do not establish a new parser's conformance. Use RFC
vectors and [JSONTestSuite](https://github.com/nst/JSONTestSuite): y_ inputs
accepted within declared limits, n_ rejected, every i_ classified under the profile.
The harness must state its input-encoding boundary and distinguish malformed
syntax from valid-but-budget-refused input. Add differential tests against a
strict declared reference plus targeted fuzzing of token boundaries, escapes,
numbers, duplicates and depth. No such new-parser run has happened yet.

### 4. Measure the indexing tradeoff

Rust provides ordered-map precedent; .NET deliberately accepts sequential
lookup to reduce initial cost; Python/JS/Go use ordinary runtime containers.
Keep sorted/contiguous indexing as a candidate, not a measured optimum.
Compare tiny/wide objects, one/many lookups, long common-prefix keys, missing
fields and complex arrays, including index construction and all native storage.
Prototype alternatives may inform one selected production design; they do not
require runtime fallbacks or hidden lazy caches. Timings need fresh authorization.

### 5. Investigate parser reuse before writing a new grammar engine

A flat DOM does not require a new parser. Assess serde's tokenizer/deserializer,
visitor and raw-value facilities or another mature tokenizer first. A normal
visitor callback is NOT proof of bounded memory: escaped-string/internal scratch
can grow before the callback. Whole-AST-then-estimate also fails the foundation's
admission contract. The implementation plan must prove bounded reservations or
hooks cover library allocations, overflow, refusals and cleanup. General-purpose
libraries do not automatically supply Hiko host-work or capability enforcement.

## simd-json owned-DOM feasibility (#264)

Base: `56b71433dfaacef21e93ee517bb867de12cd26ea`. Dev-only
[test source](../../crates/hiko-vm/tests/json_backend_feasibility.rs), not a selected
production representation. Pin `simd-json =0.19.0`, default features OFF,
`runtime-detection` + `swar-number-parsing`; serde_impl, 128bit, big-int-as-float
and approx-number-parsing OFF. Cargo.lock pins halfbrown0.4.0/value-trait0.12.3.
Crate SHA256: `e8f2e4d523646f7da6dde3983bd18ede0babc21c258ab708b3dd93ea0259719d`.

[Pre-probe freeze](https://github.com/vectorian-rs/hiko/issues/264#issuecomment-7049827486):
61 named inputs: 52 literals, six dense fixtures and depth127/128/129. Dense
arrays contain n zeroes; objects contain `"k{i}":"v"` for i=0..n-1, no whitespace,
n=64/128/256. Each test prints input name/hex for independent byte/hash checks.
Canonical manifest SHA256:
`edd3b313ac2ec4a2c27f6167b2350b46c45f6f071a3499fc4d80bb7fda36d873`.
Manifest is compact JSON in fixture order (CORPUS, each dense array/object pair,
then depths), with each row's name, bytes, lowercase hex, sha256 in that order.
The corrected run's 61 inputs match the freeze byte-for-byte. Initial generator
escaping corrupted two fixtures; that failed log remains evidence, not a backend
failure. Input bytes and acceptance requirements were not changed to fix it.

Reproduce (no timing, calibration or allocator override):

```sh
cargo +1.99.1 test --locked -p hiko-vm --test json_backend_feasibility -- --nocapture --test-threads=1
```

Source audit: [simd-json0.19.0](https://docs.rs/crate/simd-json/0.19.0/source/)
`lib.rs`, `stage2.rs`, `value/owned.rs`, `numberparse/correct.rs`,
`value/owned/serialize.rs`, and value-trait0.12.3's generator. x86 runtime dispatch
selects AVX2/PCLMUL or SSE4.2, otherwise native fallback; aarch64 uses NEON.
Local execution is aarch64; other dispatch paths are source-audited, not locally
executed. No forced-fallback or comprehensive parser-soundness test is claimed.
Owned parsing mutates caller bytes to unescape, copies them into aligned scratch,
builds a transient tape, then allocates owned strings/containers. The mutable
caller copy and scratch can die before DOM access; immutable Hiko input would
need a charged mutable copy. Owned does not mean zero-copy.

### Observed profile and fidelity blockers

Six tests pass, recording backend behavior rather than endorsing it as Hiko policy:

- Scalar/container roots and JSON whitespace accepted; second roots, suffixes,
  comments, invalid numbers/escapes/controls, trailing commas and later malformed
  values rejected before root publication. This is a targeted corpus, not JSONTestSuite/fuzzing.
- Literal Unicode and paired surrogates accepted; lone surrogates in keys, values
  and discarded duplicates rejected. Invalid UTF-8 rejected at the byte API;
  public Hiko strings cannot contain it. BOM/NBSP rejected; BOM reports backend
  InternalError, not a finished Hiko diagnostic/profile decision.
- Configured depth128 accepts127/128 and refuses129 with DepthLimitExceeded;
  backend default is1024. This does not silently adopt Hiko's proposed profile.
- u64 above i64 remains exact; checked i64 extraction fails. But `1.00`/`1.0`
  and `1e3`/`1000.0` lose spelling distinctions; integer `-0` writes `0`.
  `9007199254740993.0` rounds to9007199254740992; `1e-400` becomes0.
  Above-u64 integers, a100-digit integer and ±1e400 are refused, although valid
  JSON syntax. Owned numeric nodes cannot implement exact number_text/writing.
- Small duplicate objects retain both decoded equal keys and lookup returns the
  first, not proposed last-wins. OwnedDeserializer::parse_map uses unchecked
  insertion for at most32 members, normal deduplicating insertion above32.
  The small case is executed; the larger branch is source-audited. This is neither
  the foundation's unique sorted index nor a uniform last-wins representation.

### Capacity/admission and lifetime findings

Dense observations below are capacities/lengths, NOT allocation peaks or RSS.
A/O means array/object; caller copy capacity equals its byte length locally.

| n | Copy bytes A/O | Structural length A/O | Tape slots A/O | DOM slots A/O | Output capacity A/O |
|---:|---:|---:|---:|---:|---:|
| 64 | 129/631 | 129/257 | 129/257 | 64/112 | 1024/1024 |
| 128 | 257/1299 | 257/513 | 257/513 | 128/224 | 1024/2048 |
| 256 | 513/2707 | 513/1025 | 513/1025 | 256/448 | 1024/4096 |

Both zero/full input hints and fresh/reused buffers succeed; structural length
exceeds initial hint/128 capacity. Tape and OwnedValue slots are24 bytes locally;
object key/value string capacities total246/530/1170 bytes. Object slot counts
are not bucket bytes. Private scratch capacities, allocator metadata, native
stack and old+new growth overlap are unmeasured; these columns cannot be summed
into total native memory. The tape is a separate parse, not the owned-path peak.

**Admission NO-GO as-is:** Buffers::with_max_depth uses infallible capacity
constructors; fill_tape reserves string/index storage and replaces aligned input;
build_tape reserves tape/stack; owned construction allocates Vec/Box/map/String.
AlignedBuf uses alloc/handle_alloc_error, not fallible Hiko-budget refusal.
DumpGenerator also owns/grows output; a checked writer could bound output alone,
not repair parse admission. No caller quota/allocator hook covers these sites.
A conservative up-front byte estimate alone does not make growth fallible or
charge every old+new reservation and work boundary. No such integration is proved;
post-parse capacities are NOT a memory-refusal test. No quota was raised to pass.

The safe Rc-owner/owned-typed-path prototype demonstrates child survival after
root/input/scratch drop, literal fields, array positions, null/empty distinction,
and final owner release. No self-reference, leaked allocation, unsafe code or
borrowed-to-static cast is used in the test. It retains the WHOLE DOM, replays
O(depth) lookups and copies path bytes; it is not a constant-time arena node view
or a Hiko GC test. Production still needs traced owner/input edges as applicable,
charge-once native retention, metered views/indexes, failure/cancellation cleanup,
and last-collected-view release. Rc's non-Send is not a VM sendability check.

**Decision:** NO-GO for OwnedValue as-is; stop before #265. Prefer investigating
a mature backend/raw-token and fallible-allocation adapter with explicit hooks
under a revised approved plan. A fork/upstream change or different backend needs
review; a number-range/token compromise is a separate contract decision, not a
workaround. No custom parser, hidden cache or narrowing path was added. If those
gates are solved, the minimal provisional surface remains parse/kind/field/item/
length/scalar extraction/number_text; writing and independent detach stay #266.
No API/profile is frozen here. Existing #256 fixtures, 1e9 guards and all legacy
APIs/pins remain unchanged. No native speed, memory win or runtime readiness claim.

## Outcome and remaining authority

Retain the opaque owner/views, explicit whole-owner retention, exact tokens,
checked extraction, independent detach, native/scratch admission and compatibility
boundaries. API names, sorted indexing, integer-token-only conversion, underflow
errors, narrow capability leaves and non-sendability are still Hiko decisions,
not standards requirements endorsed by this survey.

The foundation and this comparison remain design/feasibility research.
That does not authorize production native code or settle open parser decisions.
No existing APIs, numeric behavior, grants, budgets, examples or pins change.
