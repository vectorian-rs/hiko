# Register VM: plan (2026-11, #155)

Hiko's VM moves from a stack machine to a register machine as the last
task of the 2026-11 performance series, after #151 (dispatch state), #152
(superinstructions, calls) and #153 (allocation). The maintainer decided
this in #155. This note sets out the gain, the cost, the staging that keeps
the risk low, and what to measure along the way. Background:
[`2026-11-interpreter-research.md`](2026-11-interpreter-research.md),
section 2.

**Why last:** the earlier steps change the same dispatch loop, and #152's
opcode counts tell the register design which instructions matter. Building
the register VM on top of them means designing it once.

## Where Hiko stands

After #149, #159, #164 and #165, Hiko is about 1.1x (`sieve`) to 7.7x
(`list`) slower than Lua 5.5 on `benchmarks/compare/`
(`docs/architecture/performance.md` has the tables). Lua 5 and Erlang's BEAM
are register machines; LuaJIT's interpreter is too.

## What a register VM changes

In a stack VM, every operand is moved to the stack before an instruction
uses it, and every result is pushed. `x + y` with both in locals is
`GetLocal x; GetLocal y; AddInt` (three dispatches, two stack writes, two
reads, one write). A register VM names the locals directly: `AddInt r_dst,
r_x, r_y` (one dispatch).

- **Gain.** Shi, Casey, Ertl and Gregg ("Virtual Machine Showdown: Stack
  versus Registers", VEE 2005) found that translating stack bytecode to
  registers removed nearly half of the executed instructions, for somewhat
  larger code, and ran markedly faster. wasmi, a Rust WebAssembly
  interpreter, reported large speedups when it moved to register-based
  internal bytecode in 2024.
- **Where the gain comes from.** Only from instructions that move values:
  `GetLocal*`, `SetLocal*`, `Pop`, and the stack traffic of calls. Work
  instructions (arithmetic, allocation, pattern tests, builtin calls) remain.
  So the upper bound on the gain is the dynamic share of moving
  instructions. #165 measured it with the `opcode-stats` feature: 54% of
  the 1.12 billion instructions executed by ten benchmark programs, and 63
  to 67% on `tak`, `sieve`, `nbody`, `list` and `queens`.

## What it would cost

- **Compiler:** a register allocator over Hiko's locals and temporaries,
  replacing the stack-depth bookkeeping in code generation.
- **VM:** a new `dispatch` with register operands; frames become register
  windows; calls pass arguments in registers.
- **Verifier:** rewritten. Today it tracks stack depth per instruction; a
  register verifier must track which registers are defined at each
  instruction and check every operand index. This is the security boundary
  for every program, and #143's fuzzers found two bugs in the current one in
  their first hour.
- **Everything around it:** effects and handlers (`InstallHandler` names
  stack slots for provider closures), the threaded runtime's suspension of
  frames, `inspect-work` and the work accounting (which counts executed
  opcodes, and would count fewer).

**Staging that limits the risk** (wasmi's approach): keep the current
bytecode and verifier as the program format, and translate verified stack
bytecode into register code at load time. The compiler and the verifier stay
as they are; only the translator and the new dispatch are new, and the
translator can be fuzzed for equivalence against the stack VM with the
existing targets.

## Staging

1. **Re-measure the instruction mix** with `opcode-stats` after #165's
   superinstructions (the 54% above was measured before them). This sizes
   the expected gain and picks the first instructions to translate.
2. **Register IR and translator.** Translate verified stack bytecode into
   register code at load time, function by function. The compiler and the
   bytecode verifier stay as they are, so the security boundary does not
   move in this step. A register instruction set covers the hot opcodes
   first; other functions keep running on the stack VM.
3. **Equivalence fuzzing.** Run every program on both VMs and compare
   results and errors: a new fuzz target next to #143's, seeded from the
   repository's programs. The translator must keep what the verifier proved
   (stack depths become register indices that are in range by
   construction), and the fuzzer checks it.
4. **Switch over** once every opcode translates and the suite and fuzzers
   agree; keep the stack VM behind a flag for one release, then remove it.
5. **Later, optionally:** have the compiler emit register code directly and
   verify that, once the format has settled.

## What changes for work limits

`max_work` counts executed opcodes, and a register VM executes fewer of them,
so the same program uses less work. Either scale the work charged per
register instruction, or document the change; decide in the plan PR.

## Done when

- Every program in the repository runs on the register VM with the same
  results, under the equivalence fuzzer for 30 minutes per target.
- `scripts/compare.sh` and the existing suite show the gain, recorded in
  `docs/architecture/performance.md`.
- The verifier, fuzzing and security docs describe the translator's role.
