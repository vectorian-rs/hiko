# Experimental Rust on-demand JSON reader

Implemented in [hiko-json](../../crates/hiko-json/src/lib.rs) for
[#264 rev2](https://github.com/vectorian-rs/hiko/issues/264), not registered in Hiko.
There is no Std.JsonReader/JsonDoc module, builtin, capability, GC object or new VM ABI.
Existing text/ADT APIs and #256 fixtures/1e9 guards remain unchanged. #265 rev3 is
approved; its shared-core/owned-adapter section is implemented here, not yet the
VM integration. This crate is experimental and publish=false.

## Model and API

This is a native Rust borrowing, forward-only reader over jiter0.19.0 (MIT),
default features disabled. Jiter has public pull primitives, raw numeric ranges,
caller-provided reusable string scratch and SIMD/SWAR helpers. We reuse those;
we do not port all of C++ simdjson or claim its structural-index algorithm/speed.
No OwnedValue/JsonValue, full tape, tree, persistent view, rewind or cache is built.
Jiter's variable named tape is a decoded-string byte buffer, not a parsed DOM tape.
See the [document proposal](json-document.md) for the separate owner/view design
and [comparison](json-document-comparison.md) for the retained rev1 OwnedValue NO-GO.

Reader borrows immutable input and exclusive Scratch. A root Value lends an
Object/Array; next_field returns a Field guard with key()/value(), and next_value
lends an array value. Only one outstanding child can advance the shared cursor.
Rust compile-fail doctests enforce this, including decoded-string borrow lifetimes.
find_next_field searches REMAINING entries, not earlier ones. Duplicates appear
in source order. No last-wins/sorted-object behavior is implied by this stream API.
A nested field's decoded key stays valid until its Field dies, including after
nested key decoding. Keys use a single append/truncate stack, not a buffer per key.

Value operations: kind, null, bool, string, number_text, i64, u64, f64, object,
array and skip. Scalars are consumed once. kind is a non-consuming classification,
not proof of valid token contents. number_text borrows original input; string
borrows decoded scratch through its value guard and cannot outlive it.
Owned application outputs/copies are the caller's responsibility, not reader scratch.

```rust
use hiko_json::{Limits, Reader, Scratch};
fn main() -> hiko_json::Result<()> {
let mut scratch = Scratch::default();
let mut reader = Reader::new(
    br#"{"user":{"age":37},"unused":[1,2,3]}"#,
    &mut scratch, Limits::default(),
)?;
let mut root = reader.root()?.object()?;
{
    let mut field = root.find_next_field("user")?.unwrap();
    let mut user = field.value()?.object()?;
    {
        let mut age = user.find_next_field("age")?.unwrap();
        assert_eq!(age.value()?.u64()?, 37);
    }
    user.finish()?;
}
root.finish()?;
reader.finish()?;
Ok(())
}
```

Consume a scalar, or explicitly skip/finish a child, before advancing its parent.
Dropping an unfinished child poisons the reader: Drop performs no hidden parsing
or fallible work. A poisoned reader reports an error, never quietly resumes.
After dropping Reader, Scratch is reusable even following errors; no document
references are cached. Input must remain alive while any raw numeric slice needs it.

## Validation and conversion

Construction checks input length, work/scratch admission and UTF-8, not JSON syntax.
This UTF-8 pass matters: jiter's StringDecoderRange explicitly omits that check.
Access validates consumed syntax; skip validates its entire subtree. Successful
Reader::finish drains an unopened root or requires its opened root completed,
then checks EOF. Merely extracting a field or dropping a reader does NOT validate
unread suffixes. A suffix error can follow successful extraction of earlier data.

Strict whitespace is SP/HT/LF/CR. BOM, comments, nonfinite number literals, trailing
commas, invalid escapes/controls and unpaired surrogates are rejected. All duplicate
values are validated if traversal/finish reaches them. Errors carry absolute input
byte offsets and structured kinds, not input contents. Max container depth128 counts
the root as1; public primitives plus a fixed stack enforce the SAME bound during
explicit navigation and skip. Jiter's aggregate skip/DOM methods are never used.

number_text preserves spelling and range: 1.00, -0, uint above i64 and huge exponents.
i64/u64 accept integer tokens only and check range; 1.0 is not an integer token;
-0 reads as 0 for both.
f64 is explicitly approximate, rejects nonfinite output and nonzero mantissas that
underflow to zero; genuine signed zero is retained. No implicit numeric narrowing.
RFC 8259 leaves number range to implementations. Refusing underflow is deliberate:
JavaScript, Python and serde_json return 0 for 1e-400. No relaxed mode; callers
needing another policy read number_text.

## Admission model and source audit

Defaults: max input1MiB, max estimated scratch4MiB, depth128, work1e9; these are
new standalone limits, not modifications to VM quotas. Smaller explicit limits
exercise refusal. Caller-owned borrowed input bytes have a separate input limit;
allocator metadata/rounding, code/static tables and application-owned output are
outside this scratch model. It is not RSS or an exact physical/native-stack bound.

Before parsing, reserve a checked canonical work charge of32*n+256 for n input
bytes, plus one BEFORE each frontend core request and each backend helper call.
Guard constructors/key access/completed-guard end checks do not enter the core;
api_calls counts core entries, not every Rust public method invocation.
Backend calls include requested float conversion; classification peeks are cached
per pending value, including the peek returned by array advancement. Stats report
API calls, backend helpers, classification peeks and float conversions separately.
This revised shared-core counter protocol supersedes #264's helper counts; it is
NOT the VM host-work tariff. This conservative abstract
schedule covers a UTF-8 pass, forward token scanning/validation, decoding, active
key copies/comparisons and scalar conversion. Syntax bytes progress only forward;
no root reparsing or repeated field rewind occurs. Key lengths/copies sum to at
most input size. Numeric conversion consumes its token once; fixed-precision float
fallback examines bounded significant data after scanning its token. Repeated kind
calls spend call credits even without advancing. These credits are not CPU cycles,
wall time or calibrated production Hiko host charges; no speed claim is made.

Two buffers are reserved fallibly to at least n bytes each: jiter decoded strings
and active decoded keys/temporary returned strings. Source audit of jiter0.19.0 string_decoder.rs shows clear,
reserve(min(remaining,8192)) and decoded writes no larger than raw source. All live
keys and a current decoded value belong to disjoint consumed source ranges, so
their summed decoded lengths are at most n. Neither buffer needs hidden growth under the pinned call paths.
No aggregate JsonValue/NumberAny/BigInt path or optional num-bigint feature is used.
Typed entry points check kinds first: upstream wrong_type/wrong_num otherwise invokes
NumberAny even while reporting a string/bool/null type error on numeric input.
Float conversion uses lexical-parse-float1.0.6 fixed-limb StackVec fallback; inspect
number_decoder.rs and lexical bigint.rs rather than assuming float means allocation.

The scratch charge includes both retained capacities, old+new overlap when replacing
each buffer, fixed Rust reader/guard payloads and shared frames at depth128, plus a canonical
64KiB allowance for fixed decoder/conversion payloads. This allowance is an estimate,
not a compiler frame-size measurement. Preflight computes the sequential replacement
peak before allocation; replacements use fresh try_reserve_exact buffers. OS failure
may leave an already-grown buffer reusable but never leaks it. Every backend lease
returns jiter's buffer to Scratch, including on syntax errors; Drop clears contents. No global allocator override or
allocate-first/estimate-later budget path is used. Upstream code contains unsafe;
this wrapper forbids new unsafe code and performs no lifetime widening.

## Shared core and owned frontend (#265 rev3 S1)

Both Reader and OwnedReader use one input-independent core: absolute offset,
cached classification and fixed128-frame traversal state. Lending guards retain
only completion bookkeeping. Public Jiter leases operate on input[offset..],
return tape before the next operation, and never replay a prefix or retain a
borrow of their frontend. Jiter's Parser is private and is not used directly.
No self-reference, unsafe lifetime widening, backend change or extra grammar.
Decoded strings are copied into pre-reserved scratch, then lent to the caller;
raw numbers still borrow original input. No copied borrowed input or DOM cache.

OwnedReader retains Arc<str>, scratch and core state, and can safely move between
operations. next_field/next_item end markers leave the frame open until matching
finish_object/finish_array. A pending child must first be consumed; remaining-field
search skips unmatched values. finish drains all open frames/root and checks EOF.
This standalone Rust frontend permits repeated finish; the approved Hiko finish
will instead release its owner, so subsequent finish fails. Hiko close will discard,
not validate. No persistent VM Poisoned/Validated states are introduced by S1.

admission_bytes is a conservative fresh-construction estimate covering input,
two buffers and fixed state/decoder allowance. estimated_bytes uses retained
capacities. Both are canonical estimates, not allocator profiling or RSS. A VM
must reserve BEFORE construction/output work; this standalone adapter is not that
reservation ledger. Runtime object_bytes/slot release/GC/authority integration
remains S2–S4, not a source-inspection proof. The approved VM starting tariff is
json_parse's45+ceil(2180*n/1024) once, then45 per subsequent API call, not32*n.
It is an existing-rate proxy, not newly calibrated reader performance.

## Reproduction and limitations

```sh
cargo +1.99.1 test --locked -p hiko-json -- --nocapture --test-threads=1
cargo +1.99.1 clippy --locked -p hiko-json --all-targets -- -D warnings
```

[Pre-probe freeze](https://github.com/vectorian-rs/hiko/issues/264#issuecomment-7067454829):
crate SHA2564027f86130c7a1997f880ed407580e58514b867af49e7ce44370f80fa6ff999f;
manifest SHA25600fa4afbb5d217b8a92b0e5a96ff92f432548ea47f97ff0a9555634089dbcba8.
Tests retain the original61 byte/hash inputs (large-number acceptance changes only
in this NEW raw reader), plus fixed reader fixtures, depth127/128/129, dense64/128/256,
long escaped keys/strings, admission/poison/reuse failures, shared-profile serde_json
differential checks and4096 deterministic generated byte inputs. This is not a
JSONTestSuite/full-conformance or fuzz-campaign claim. No timed benchmark runs.
Resource evidence is source audit plus visible capacity/cursor counters, not a
complete native-allocation profiler. These local guards are not yet VM accounting,
GC/cancellation, capability, process-transfer or production host-charge integration.
