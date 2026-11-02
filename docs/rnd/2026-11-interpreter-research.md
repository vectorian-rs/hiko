# Interpreter research that applies to Hiko (2026-11)

Which published techniques could make Hiko's VM faster, and how each fits a
minimal, verified, security-first VM. Companion to
[`2026-11-vm-performance-review.md`](2026-11-vm-performance-review.md), which
profiles the VM, and to `docs/architecture/performance.md`, which records the
measurements.

The references were written from memory and have not been re-checked against
the publications. Verify titles, venues and figures before citing them
elsewhere.

## Where Hiko stands

Since #146, #149 steps 2 and 3 removed per-instruction call and error-path
overhead: Hiko is 8 to 28% faster and now 1.3x (`sieve`) to 11.3x (`list`)
slower than Lua 5.5. What remains is the work per instruction (frame lookups,
bounds and tag checks), the number of instructions (a stack machine pushes
and pops what a register machine names directly), and allocation in
list-heavy code.

Hiko's position differs from the dynamic languages most of this research
targets in three ways:

- **Static types.** The type checker already knows operand types, so run-time
  specialization (quickening, inline caches on types) matters less, and
  compile- or load-time specialization is possible.
- **Verified bytecode.** Every program passes the bytecode verifier before it
  runs, as in WebAssembly. Facts the verifier proves (stack depths, jump
  targets, local slots) could replace run-time checks, at a security cost.
- **Immutable data and processes.** Values are immutable (closures for
  recursion aside), and processes copy the values they send, as in Erlang.

## 1. Dispatch: keeping the interpreter's state in registers

**Tail-call interpreters.** Each opcode handler is its own function that
tail-calls the next handler. The compiler keeps the instruction pointer,
stack pointer and frame in machine registers across handlers, and every
handler ends in its own indirect jump, which predicts better than one shared
`match`.

- Josh Haberman, "Parsing Protobuf at 2+GB/s: How I Learned To Love Tail
  Calls in C" (2021), with Clang's `musttail`.
- CPython 3.14 added a tail-calling interpreter built with Clang. Its first
  reported speedup was later found to be overstated, because of a compiler
  regression in the baseline; the gain that remains is smaller but real.
- wasm3's "M3" interpreter uses tail calls and also passes the top of the
  stack in a register.
- Rust: explicit tail calls (`become`, RFC 3407) are on nightly. On stable,
  the Rust Wasm interpreter Stitch relies on LLVM's sibling-call optimization,
  which is not guaranteed.

**Stack caching.** M. Anton Ertl, "Stack Caching for Interpreters" (PLDI
1995): keep the top one or two stack values in locals instead of memory.
This applies to Hiko's stack VM without a redesign.

**Generated interpreters.** Haoran Xu and Fredrik Kjolstad, "Deegen: A
JIT-Capable VM Generator for Dynamic Languages" (2023): handlers generated
with LLVM, with pinned registers and tail calls. Its generated Lua
interpreter outperforms LuaJIT's interpreter.

**For Hiko:** #149 step 4 (`ip`, code and frame base held in locals in the
dispatch loop) gets much of this in stable Rust. Stack caching can follow.
A handler-per-opcode design with `become` is the later form, once explicit
tail calls are stable.

## 2. Fewer instructions: specialization and registers

**Superinstructions.** M. Anton Ertl and David Gregg, "Optimizing Indirect
Branch Prediction Accuracy in Virtual Machine Interpreters" (PLDI 2003), and
their survey "The Structure and Performance of Efficient Interpreters" (JILP
2003): fuse frequent instruction sequences into one instruction, and
replicate hot instructions. Choose the sequences from measured pair counts.

**Quickening and specializing interpreters.** Stefan Brunthaler, "Efficient
Interpretation using Quickening" (DLS 2010); CPython's specializing adaptive
interpreter (PEP 659, Python 3.11). Instructions rewrite themselves into
specialized forms at run time. Hiko can specialize at compile time from
static types (it already has typed opcodes such as `AddInt`), or at load
time like Erlang's BEAM loader, which turns generic instructions into many
specialized ones and superinstructions.

**Register machines.** Yunhe Shi, Kevin Casey, M. Anton Ertl and David Gregg,
"Virtual Machine Showdown: Stack versus Registers" (VEE 2005; TACO 2008):
translating stack bytecode to registers removes nearly half of the
instructions executed, for somewhat larger code, and runs markedly faster.
Lua 5 and BEAM are register machines. The closest precedent for Hiko is
wasmi, a Rust WebAssembly interpreter that validates its input. It moved from
stack-based to register-based internal bytecode in 2024 and reported large
speedups.

**For Hiko:** superinstructions guided by opcode-pair counts are the next
step after step 4; each new opcode needs a verifier rule and fuzzing. A
register VM is the structural change if a large gap remains; it rewrites the
compiler, the VM and the verifier, so it would be a plan of its own.

## 3. Using the verifier

**In-place interpretation of validated code.** Ben L. Titzer, "A Fast
In-Place Interpreter for WebAssembly" (OOPSLA 2022): the validator produces a
side table (control-flow targets, stack heights), and the interpreter runs the
original bytecode using it, with no rewriting step. Hiko's verifier already
computes these facts.

**Baseline compilers.** Ben L. Titzer, "Whose Baseline Compiler Is It
Anyway?" (CGO 2024), on single-pass compilers. Haoran Xu and Fredrik
Kjolstad, "Copy-and-Patch Compilation" (OOPSLA 2021): a fast template JIT
from precompiled code stencils, which CPython 3.13 adopted for its
experimental JIT.

**For Hiko:** a verifier side table is a cleaner basis than ad hoc trust for
removing run-time checks (review item 8). But a verifier bug then becomes
memory unsafety rather than an error, and #143's fuzzers found two verifier
bugs in their first hour. A JIT, even copy-and-patch, is outside the minimal
and auditable goal; listed for completeness.

## 4. Memory: reuse and per-process heaps

**Precise reference counting with reuse.** Alex Reinking, Ningning Xie,
Leonardo de Moura and Daan Leijen, "Perceus: Garbage Free Reference Counting
with Reuse" (PLDI 2021, used in Koka); Sebastian Ullrich and Leonardo de
Moura, "Counting Immutable Beans: Reference Counting Optimized for Purely
Functional Programming" (IFL 2019, used in Lean 4). When a cell is uniquely
owned, `map` or `filter` can overwrite it instead of allocating a new one
("functional but in-place"). This fits Hiko's immutable ML-style data
unusually well and targets the allocation in `list` and `binary_trees`. It
needs acyclic data: Hiko's values are acyclic except recursive closures
(`MakeRecClosures`), which would need the treatment Lean gives similar cases.

**Tracing collectors.** Stephen M. Blackburn and Kathryn S. McKinley,
"Immix: A Mark-Region Garbage Collector with Space Efficiency, Fast
Collection, and Mutator Performance" (PLDI 2008); Wenyu Zhao, Stephen M.
Blackburn and Kathryn S. McKinley, "Low-Latency, High-Throughput Garbage
Collection" (LXR, PLDI 2022).

**Per-process heaps.** Erlang's BEAM gives each process its own generational
copying heap and copies messages between heaps. Hiko already copies values
between processes; its immutability means old objects almost never point to
new ones, which makes a young generation cheap.

**For Hiko:** a Perceus-style reuse experiment on `list`/`binary_trees` is
the most Hiko-specific bet here and could matter more than dispatch work for
allocation-heavy code. It is research-grade: an experiment and a note first.

## 5. Code layout

Maksim Panchenko, Rafael Auler, Bill Nell and Guilherme Ottoni, "BOLT: A
Practical Binary Optimizer for Data Centers and Beyond" (CGO 2019):
profile-guided reordering of a linked binary; CPython builds use it. #149
saw layout effects directly (a 1 to 2% loss on `list` from an unrelated
change, and fat LTO slowing the interpreter down). BOLT is a build step with
no code change, worth one experiment after the dispatch work.

## Suggested order

1. #149 step 4: `ip`, code and frame base in locals; then stack caching.
2. Superinstructions from measured opcode-pair counts.
3. Cheaper calls (no reference-count traffic per call; review item 6).
4. BOLT, as a build experiment.
5. Perceus-style reuse, as an R&D experiment on `list` and `binary_trees`.
6. A register VM, following wasmi, only if a large gap remains.
7. Not planned: a JIT; trusting the verifier for unchecked access without a
   separate decision on the security trade-off.

Each step is measured as in `docs/architecture/performance.md`: interleaved
A/B runs on AC power, kept only if it pays.
