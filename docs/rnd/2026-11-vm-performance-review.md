# VM performance review (2026-11)

Where Hiko's interpreter spends its time, and what to change, in order of
gain per unit of effort and risk. Measured on `main` after #139; the
comparison numbers are in `docs/architecture/performance.md` ("Against Lua
and Python", #146).

## The gap

Hiko is 2 to 14 times slower than Lua 5.5 and 2 to 10 times slower than
Python 3.13 on the eight programs in `benchmarks/compare/`. Start-up is not
the cause (4.9 ms for a one-line program, against 3.1 ms for Lua).

## Where the time goes

macOS `sample`, 1 s of each run, release build, share of the VM thread's
samples by the function on top of the stack:

| Program | `dispatch` | `read_u16` | `Op::try_from` | stack index helpers | GC and allocation | building data values |
|---|---|---|---|---|---|---|
| `list` | 48% | 18% | 14% | 8% | 1% | - |
| `binary_trees` | 46% | 7% | 11% | 6% | 13% | 9% |
| `sieve` | 43% | 12% | 8% | 10% | 7% | 12% |
| `nbody` | 50% | 10% | 7% | 11% | 2% | 7% |

The gap is interpretive overhead, not memory management. Helpers that run on
every instruction are not inlined and take 20 to 40% of the time. GC and
allocation take at most 13%.

## Opportunities, ranked

Line references are to `main` before #149, which changed several of them.

Gains are estimates from the profile shares and must be measured, interleaved
and on AC power (`docs/architecture/performance.md`, "How to measure").

### 1. Release profile (no code change) — measured, not kept

Measured in #149: fat LTO with one codegen unit made 13 of 15 programs 1 to
15% slower (`docs/architecture/performance.md`). The text below is the
original estimate.


The root `Cargo.toml` has no `[profile.release]` and there is no
`.cargo/config`, so release builds use thin local LTO and 16 codegen units.
`Op::try_from` (`hiko-compile/src/op.rs:145`) is in another crate with no
`#[inline]`, so it cannot be inlined and shows as its own frame (7 to 14%).

- Proposed (and measured as slower, so not kept): `lto = "fat"`,
  `codegen-units = 1`. Kept instead: `#[inline]` on `Op::try_from`.
  Keep `panic = "unwind"`: `io_backend.rs:534` relies on `catch_unwind`.
  `target-cpu` adds nothing on aarch64-apple-darwin (already apple-m1). PGO
  (`cargo-pgo`) after the rest.
- Expected: 5 to 15%. Risk: none; slower release builds.

### 2. Build data values without `drain().collect()`

`dispatch.rs:252, 266, 272` build tuples, constructors and records with
`self.stack.drain(start..).collect::<Fields>()`, the `SmallVec::extend` frame
(7 to 12%).

- Change: `Fields::from_slice(&self.stack[start..])` then
  `self.stack.truncate(start)` (`Value` is `Copy`, so this is a copy of a
  slice); `MakeCons` pops twice and uses `Fields::from_buf([a, b])`.
- Expected: most of the 7 to 12%. Risk: none.

### 3. Inline hot helpers; move error paths out of line

`RuntimeError` holds a `String` (24 bytes, `vm/mod.rs:56`), so every
`Result<_, RuntimeError>` is returned through memory. `read_operand`,
`push`/`pop` and `local_stack_index` build their `format!` errors inline, so
the inliner declines them.

- Change: `#[inline(always)]` on the hot helpers; errors built in
  `#[cold] #[inline(never)]` functions; `RuntimeError` boxed (8 bytes) behind
  a `message()` accessor.
- Expected: 10 to 20% (most of `read_u16` and the index helpers). Risk:
  small public API change to `RuntimeError`.

### 4. Keep `ip`, `code` and `base` in locals

Every instruction re-derives the frame (`frames.len() - 1`, `frames[fi]`,
`chunk_for_checked`, an `ip` bounds check, an `ip` store,
`dispatch.rs:30-42`), and every operand read and local access repeats part of
it.

- Change: hold `Arc` clones of the protos and the main chunk for the
  duration of `dispatch`, so the code slice does not borrow `self`; keep `ip`,
  `code` and `base` in locals; reload on call, return, handler and suspend;
  write `ip` back before anything that reads it. (This is not the reverted
  "loop-head fetch via `read_operand`", which made the fetch heavier.)
- Expected: 20 to 35% on call- and local-heavy programs (`list`, `sieve`).
  Risk: medium refactor, no `unsafe`; a stale `ip` in error spans or suspends,
  which existing tests cover.

### 5. Cheap allocation fixes

- `values_equal` allocates a `Vec` worklist for every heap-to-heap
  comparison (`vm/mod.rs:233`), including string `=`: compare two strings
  directly, and use a `SmallVec` worklist.
- `string_cache` is a SipHash `HashMap<(usize, usize), GcRef>` looked up on
  every string `Const` (`dispatch.rs:66`): a per-proto `Vec<Option<GcRef>>`.
- With `max_heap` set, every allocation does an atomic compare-and-swap
  (`heap.rs:1107`), and the size estimate and object-limit check run twice
  (`gc.rs:15`, `heap.rs:1097`): reserve budget in 64 KiB chunks with the
  existing `reserve_up_to` and refund the rest at GC or exit.
- GC threshold floor `max(1024)` (`heap.rs:1242`) is small: try 32K objects;
  store marks as a bitset.
- Expected: a few percent overall, a real share of `binary_trees`' 13%. Risk:
  low; the budget tests must still pass with chunked reservation.

### 6. No reference-count traffic per call

Every call clones an `Arc<[Value]>` of captures, even the shared empty one
(`dispatch.rs:435, 484, 657`): an atomic increment and decrement per call.

- Change: `CallFrame.captures` becomes the closure's `GcRef` (the heap does
  not move objects; closures are immutable after `MakeRecClosures`).
- Expected: 3 to 6% on `ack`, `tak`. Risk: medium; GC roots change and
  `GetUpvalue` gains a heap lookup.

### 7. Superinstructions from an opcode-pair histogram

Count opcode pairs (behind a feature flag) on the suites, then fuse the top
ones, for example `JumpIfLocalNotTag`, `GetLocalN; GetField0/1`,
`GetLocal; GetLocal; AddInt`. Expected 5 to 15% on matching programs. Each new
opcode needs a verifier rule and fuzzing.

### 8. Unchecked access justified by the verifier (last, if at all)

The verifier proves opcodes, operands, jump targets, stack depth and local
slots (since #143, that a local slot is on the stack). About three sites would
need `unsafe` (`get_unchecked` for fetch, local read, pop), with
`debug_assert!`s and a `Verified` token, and dispatch must refuse to run an
unverified program. After 3 and 4, a predicted bounds check costs about a
cycle: expected 3 to 8%. The security cost (verifier bugs become memory
unsafety; #143 found one such bug in its first hour) likely outweighs it.

### 9. Larger designs

- **Generational nursery.** The heap is immutable except
  `set_closure_captures`, so old-to-young pointers can only come from there;
  the write barrier would be one call site. Needs a separate young index
  range. High effort; targets the GC share.
- **Register VM.** 30 to 45% fewer instructions in the literature; rewrites
  the compiler and the verifier.
- **Threaded dispatch.** Not available in stable Rust (no computed goto;
  `become` is unstable). Apple's indirect branch predictor reduces the gain.

## What not to do

- **Change `Value`'s representation.** It is 16 bytes, `Copy`, and size-tested
  (`value.rs`), like Lua's `TValue`. NaN-boxing to 8 bytes would need boxed or
  narrower 64-bit `Int`/`Word`/`Pid`, a language change.
- **SIMD in the interpreter.** Dispatch is bound by branches and memory, not
  arithmetic.

## SIMD, NEON and AVX: where they apply

- Already vectorized: `blake3` (NEON on aarch64; SSE4.1/AVX2/AVX-512 with
  runtime detection on x86), `regex` (through `memchr` and `aho-corasick`),
  `chars().count()`.
- Candidate: `string_contains`, `split`, `string_replace` (`string.rs`) use
  `str` search, which has no SIMD prefilter on aarch64; `memchr::memmem` does.
  Helps long haystacks only. `memchr` is in the tree only with
  `builtin-regex`.
- Related: an "is ASCII" flag set at string creation would make
  `string_length` and `substring` O(1) on ASCII strings.

## Order

1, 2, 3 and 5 first: small, safe, perhaps 25 to 40% together on the profiled
programs. Measure each on `benchmarks/compare/` and the existing suite. Then
4, re-profile, and only then 6 to 9.

Coordination: AB (#147) edits `value.rs`, `heap.rs` and `vm/mod.rs`, not
`dispatch.rs` or `verify.rs`. Items 1, 2 and 4 can go ahead; 3 changes
`RuntimeError` in `vm/mod.rs` (a small overlap); 5 and 6 touch `heap.rs` and
`vm/mod.rs` and should follow AB.
