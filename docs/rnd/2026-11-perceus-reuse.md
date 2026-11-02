# Perceus-style reuse for Hiko (2026-11, #154)

Whether precise reference counting with in-place reuse (Perceus in Koka;
"Counting Immutable Beans" in Lean 4) could remove most allocation from
list- and tree-heavy Hiko programs, and what it would cost. This note answers
the questions in #154 from the current code; it has no prototype yet.
Background and references: [`2026-11-interpreter-research.md`](2026-11-interpreter-research.md),
section 4.

**Recommendation: no-go for now, revisit after #153.** Reuse fits Hiko's
data model well, but replacing the tracing collector is a change to the
heap, the compiler, the verifier and the process model at once, and the
measured cost of memory management today (1 to 13% of run time) is smaller
than what the dispatch work (#151, #152) is still recovering. A narrow
experiment that keeps the tracing collector is described at the end.

## How memory works today

- `Heap` (`hiko-vm/src/heap.rs`) is a per-process arena: `objects:
  Vec<Option<HeapObject>>`, a parallel `marks: Vec<bool>` and
  `object_bytes`, and a `free_list` of slot indices. A `GcRef` is a `u32`
  slot index; objects never move.
- Collection is mark-and-sweep with an explicit worklist from the roots (the
  stack, frame captures, handler frames, the string-constant cache), started
  when `alloc_since_gc` reaches `gc_threshold` (floor 1024 objects).
- A slot holds an `Option<HeapObject>` of at most 64 bytes (`size_tests` in
  `value.rs`); a cons cell is `Data { tag, fields: SmallVec<[Value; 2]> }`,
  so about 73 bytes with its `object_bytes` entry and mark.
- Values are immutable once built, with one exception:
  `set_closure_captures`, used only by `MakeRecClosures` to point the
  closures of a recursive group at each other after all of them exist.
- Processes have separate heaps. A message is serialized
  (`SendableValue`) and rebuilt in the receiver's heap; strings and bytes
  share their `Arc` payload.

## The questions in #154

### 1. Cycles

Ordinary values cannot form cycles: a tuple, constructor or cons cell can
only point at values that existed before it. The only cycles are recursive
closure groups built by `MakeRecClosures`.

Options, in order of preference:

- **Treat closures as not reference counted.** Count only `Data` and
  `Tuple`; closures (and the values only they reach) stay under the tracing
  collector. This is what a narrow experiment would do anyway.
- **Lean's approach.** Lean marks objects reachable from a top-level
  constant as persistent and never counts them; recursive groups here are
  usually top-level or long-lived, so they could be marked persistent when
  `MakeRecClosures` builds them.
- **Weak back edges.** Make the capture from a closure to its own group
  weak. Precise, but `GetUpvalue` would have to check liveness, a cost on a
  hot path.

### 2. Processes

Reference counts would stay per process, like the heap: a message is
already copied, so no count crosses a process boundary. `Arc` payloads of
strings and bytes keep their own atomic counts, as now. Nothing changes in
`SendableValue`.

### 3. Compiler and verifier

Perceus inserts `dup` and `drop` at the points where a variable is last
used or copied, and turns a `drop` of a cell followed by an allocation of
the same size into a reuse. In Hiko:

- The compiler knows variable scopes (it already computes `n_locals` and
  slot reuse) but not last uses; Perceus needs a liveness pass over each
  function, with care at pattern matches (each branch drops what it does not
  use) and at tail calls.
- New instructions (`Dup`, `Drop`, `ReuseCons` or a reuse token on
  `MakeData`) need verifier rules. The verifier cannot prove counts correct;
  a wrong count is a use-after-free, so counts must stay checked at run time
  (a freed slot is `None`, and `heap.get` already returns an error for it),
  turning a compiler bug into a runtime error rather than memory unsafety.
- Every builtin that holds `Value`s across allocation would need to respect
  counts, or the heap must treat builtin-held values as borrowed.

### 4. Cost model

- **Counting cost.** Every copy of a heap `Value` onto the stack (`GetLocal`,
  `GetField`, argument passing) becomes an increment, and every pop a
  decrement. Hiko's stack VM copies values much more than a register VM or
  compiled code does, so counting traffic would be higher than in Koka or
  Lean, which compile to C. This is the main risk.
- **Gain.** On programs like `list` (building and walking lists) and
  `binary_trees`, reuse can turn most allocations into in-place updates, and
  freeing becomes immediate instead of batched. GC and allocation are 13% of
  `binary_trees` and under 2% of `list` today
  ([`2026-11-vm-performance-review.md`](2026-11-vm-performance-review.md));
  so for `list` the gain must come from fewer allocations (cache behaviour),
  not from less GC time, and is uncertain.
- **Tracing stays cheap here.** Mark-and-sweep over a per-process arena with
  immutable data has no write barrier and no pause across processes; after
  #153 (allocation fixes, a larger GC threshold) its share will shrink
  further.

### 5. A narrow experiment

If revisited, the cheapest test of the idea keeps the tracing collector:

- **Uniqueness without counts.** A list built and consumed by one
  self-recursive function (`map`, `filter`, `rev` patterns) can be detected
  at compile time when the input list is a parameter used exactly once on
  each path. For those functions only, the compiler emits a reuse form of
  `MakeCons` that overwrites the matched cell instead of allocating.
- **Safety net.** The reuse instruction checks at run time that the cell is
  not reachable from elsewhere; without counts it cannot, so the experiment
  is only sound if the compiler's analysis is. That is the reason to build
  it behind a cargo feature and fuzz it with `fuzz_typecheck`, not to ship
  it by default.
- **Measure** on `list` and `binary_trees` with the method in
  `docs/architecture/performance.md`.

## Decision

- Do not replace the tracing collector now.
- Do #153 first; re-profile.
- If allocation is still a large share afterwards, run the narrow experiment
  above as its own PR behind a feature flag, with the soundness argument
  written down before the code.
