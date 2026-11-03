# Register VM: design (2026-11, #155)

How Hiko's register VM works, following the plan in
[`2026-11-register-vm-plan.md`](2026-11-register-vm-plan.md): verified stack
bytecode is translated to register code at load time, the stack VM stays as
the fallback, and an equivalence fuzzer checks the two agree. This note is
for review before any code.

## Why

On `main` (after #149, #159, #164, #165), 47% of the 960 million
instructions executed by ten benchmark programs only move values between
locals and the operand stack (`GetLocal*`, `SetLocal*`, `Pop`), and 57 to
67% on `sieve`, `queens`, `nbody` and `tak` (opcode counts,
`docs/architecture/performance.md`). A register machine names its operands,
so most of those moves disappear, and each remaining instruction does more
work per dispatch.

## Registers are the existing stack slots

A frame's register *r* is `stack[base + r]`. Locals already live at
`base + slot`, arguments at `base .. base + arity`, and temporaries above
them; the verifier already knows the maximum stack depth of every function.
So:

- a function's register count is `n_locals` plus its maximum operand depth,
  which the verifier computes today;
- a call reserves `base .. base + n_regs` on the existing value stack;
- garbage-collection roots, process suspension (`run_slice`), child-process
  spawning and effect handlers keep working on the same `stack` and
  `frames`, because the layout does not change, only how instructions name
  slots;
- `InstallHandler`'s provider closures already name local slots, which are
  registers.

Registers above a frame's live values may hold stale values; a frame clears
its registers when it returns, as the stack VM truncates today, so no stale
value outlives a call.

## Instructions are an enum, decoded once

Register code is a `Vec<RegOp>`, where `RegOp` is a Rust enum with typed
fields, for example:

```rust
enum RegOp {
    Move { dst: Reg, src: Reg },
    Const { dst: Reg, idx: u16 },
    AddInt { dst: Reg, a: Reg, b: Reg },
    JumpIfNotLtInt { a: Reg, b: Reg, target: u32 },
    JumpIfNotTag { reg: Reg, tag: u16, target: u32 },
    Field { dst: Reg, src: Reg, idx: u8 },
    MakeData { dst: Reg, tag: u16, first: Reg, n: u8 },
    Call { proto: u16, first_arg: Reg, n_args: u8, dst: Reg },
    TailCall { proto: u16, first_arg: Reg, n_args: u8 },
    Return { src: Reg },
    // ...one variant per stack opcode family
}
type Reg = u16;
```

Operands are read from the enum, not decoded from bytes, so the per-operand
decoding that #159 and #165 reduced goes away entirely. Jump targets are
instruction indices. An enum of 8 to 12 bytes per instruction is larger
than the bytecode, which costs some cache; that is measured, not assumed.

## Translation: virtualize the operand stack

The translator walks each verified function once, keeping a *symbolic*
operand stack whose entries say where a value is (a register, a constant)
instead of emitting code to move it:

- `GetLocal s` pushes "register s" and emits nothing.
- `Const k` pushes "constant k" and emits nothing.
- `AddInt` pops two symbolic operands and emits
  `AddInt { dst, a, b }`, where `dst` is the register of the stack slot the
  result occupies.
- `SetLocal s` stores the top operand into register s; any symbolic entry
  still referring to register s is first spilled to its own stack-slot
  register, so it keeps the old value.

Symbolic entries are written to their stack-slot registers (spilled) at the
points where the stack VM's layout must be real:

- before a jump and at every jump target, so all paths agree on where
  values are;
- before a call, so the arguments sit in consecutive registers;
- before an instruction the translator does not virtualize.

This is the approach of wasmi's register translation and of Lua's compiler,
and it removes most moves without a general register allocator.

The verifier stays the gate: translation runs only on verified bytecode, and
uses the verifier's stack depths and jump targets.

## Calls and returns

- `Call { proto, first_arg, n_args, dst }`: the arguments are in registers
  `first_arg ..`; the callee's frame starts there (`base` of the callee is
  the caller's `base + first_arg`), as today.
- `Return { src }` copies the result to the caller's `dst` register and
  truncates the stack to the callee's base.
- Tail calls copy arguments down to the frame's base, as today.
- Closures and upvalues are unchanged; `GetUpvalue` becomes
  `Upvalue { dst, idx }`.

## Mixed execution and fallback

Translation is per function. A function containing an opcode the translator
does not handle yet stays on the stack VM. Each frame records which code it
runs, and calls between the two kinds work because both use the same stack
layout. This lets the translator grow opcode by opcode, measured at each
step, and keeps the stack VM as a fallback (for example, a cargo feature to
disable translation) for one release after switching over.

## Checking translated code

- **No new `unsafe`.** Register accesses stay bounds-checked, so a translator
  bug gives a wrong result or a runtime error, not memory unsafety.
- **A register-code checker** (small, defence in depth) validates each
  translated function: every register index below its register count, every
  jump target a valid instruction index.
- **An equivalence fuzz target** runs each program on both VMs (translation
  on and off) and requires the same output, result and error. It is seeded
  from the repository's programs and the existing fuzz corpora, next to
  #143's targets.
- **The test suite** runs with translation on and off.

## Work limits

`max_work` counts executed opcodes, so a register VM would let the same
program do more under the same budget. Each `RegOp` records how many stack
instructions it replaced, and dispatch charges that weight. Budgets, error
points and `inspect-work` then mean what they mean today.

## Staging

1. **Register IR, translator and dispatch** for the hot opcodes: locals,
   constants, integer and float arithmetic and comparisons, branches, field
   reads and tag tests, data construction, direct calls and returns. Behind
   a cargo feature, off by default; the stack VM runs everything else.
2. **Equivalence fuzzing and dual-mode tests,** before anything is on by
   default.
3. **The remaining opcodes:** closures, upvalues, globals, builtins, effects
   and handlers, processes.
4. **Switch on by default,** measured with `scripts/compare.sh` and the
   existing suite; the stack VM stays behind a feature for one release.
5. **Remove the stack dispatch** later, if nothing needs it.

Each step is its own PR with measurements in
`docs/architecture/performance.md`, kept only if it pays.

## Open questions

- `Reg` as `u16` (simple, any function size) or `u8` with a wide form (more
  compact). Start with `u16` and measure.
- Whether to keep the enum or pack instructions into `u32` or `u64` words
  later, if cache misses show up in profiles.
- Whether translation runs once per program at load (proposed) or lazily on
  a function's first call.
