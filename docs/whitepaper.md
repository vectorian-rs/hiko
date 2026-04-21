# Local Algebraic Effects on an Isolated-Process Runtime

## Abstract

Asynchronous programming is often split between two unsatisfying approaches.
Future- and callback-based systems impose function coloring and make direct
style harder to preserve. Shared-heap effect runtimes recover direct style, but
push substantial complexity into scheduling, memory management, cancellation,
and synchronization. Hiko explores a different point in the design space:
local algebraic effects for intra-process control flow, combined with an
isolated-process runtime with per-process heaps, explicit process-boundary
transfer, and host-provided capabilities.

The interesting claim is the combination, not any one ingredient in isolation.
Local effects stay process-local, asynchronous I/O is mediated by a fixed host
runtime protocol, heaps remain isolated per process, and capabilities stay
host-owned. This produces direct-style scripting inside each process, explicit
ownership boundaries between processes, local garbage collection, and a runtime
model that is easier to reason about for safety-oriented tooling.

## Implementation Snapshot

The current implementation in this repository is already a concrete instance of
that design, not just a proposal:

- a stack-based bytecode VM with 16-byte `Copy` values and `GcRef(u32)` heap references
- mark-and-sweep GC with iterative marking and adaptive thresholds
- two runtimes: a simple single-threaded runtime and a multi-worker threaded runtime
- deep continuation capture for effect handlers
- capability checks at the VM/runtime boundary for filesystem, HTTP, and `exec`
- zero `unsafe` in `hiko-vm`

The operational target is short-lived agent/tool workloads. Typical VMs are
expected to run for seconds to minutes, so the implementation prioritizes
correctness, isolation, predictable teardown, and human readability over
long-horizon daemon-runtime optimization.

## 1. Status and Reading Stance

This whitepaper is about **language and runtime meaning**, not about freezing
surface spelling too early.

The semantics described here are substantially more stable than the exact
syntax. The current parser and [hiko.ebnf](hiko.ebnf) are snapshots, not the
final language contract. In particular:

- package-loading syntax is still settling
- some library naming may still tighten
- surface conveniences may still move if they do not change meaning

The right way to read the document is:

- trust the semantic split between local effects and runtime-managed async
- trust the isolated-process and process-boundary model
- treat exact keywords and package syntax as provisional unless they are
  already heavily exercised in the repository

### 1.1 Verification map

This document is a design summary, not the only source of truth. For actual
changes or reviews, verify each area against the maintained docs and source:

| Area                                           | Primary verification point                                                                                                                                                                                                                                                   |
| ---------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Core language surface and types                | [system.md](system.md)                                                                                                                                                                                                                                                       |
| SML divergences and simplification policy      | [sml-deltas.md](sml-deltas.md)                                                                                                                                                                                                                                               |
| Module semantics                               | [modules.md](modules.md)                                                                                                                                                                                                                                                     |
| Error layering and `Result` discipline         | [error-handling.md](error-handling.md)                                                                                                                                                                                                                                       |
| Runtime/process model                          | [runtime.md](runtime.md)                                                                                                                                                                                                                                                     |
| VM/runtime seam and process-creation cost      | [vm.md](vm.md)                                                                                                                                                                                                                                                               |
| Current `Std.Fiber` API                        | [../libraries/Std-v0.1.0/modules/Fiber.hml](../libraries/Std-v0.1.0/modules/Fiber.hml)                                                                                                                                                                                       |
| Current `Result`/`Option`/`Either` definitions | [../libraries/Std-v0.1.0/modules/Result.hml](../libraries/Std-v0.1.0/modules/Result.hml), [../libraries/Std-v0.1.0/modules/Option.hml](../libraries/Std-v0.1.0/modules/Option.hml), [../libraries/Std-v0.1.0/modules/Either.hml](../libraries/Std-v0.1.0/modules/Either.hml) |
| Runtime lifecycle and join/cancel state        | [../crates/hiko-vm/src/process.rs](../crates/hiko-vm/src/process.rs), [../crates/hiko-vm/src/runtime.rs](../crates/hiko-vm/src/runtime.rs), [../crates/hiko-vm/src/threaded.rs](../crates/hiko-vm/src/threaded.rs)                                                           |
| Process-boundary transfer                      | [../crates/hiko-vm/src/sendable.rs](../crates/hiko-vm/src/sendable.rs)                                                                                                                                                                                                       |
| VM execution transitions                       | [../crates/hiko-vm/src/vm/runtime_bridge.rs](../crates/hiko-vm/src/vm/runtime_bridge.rs)                                                                                                                                                                                     |

If this whitepaper and those files disagree, the code and the maintained
implementation docs win.

### 1.2 If you are changing something specific

Use this document for meaning, then load the narrower source-of-truth docs:

- language syntax, precedence, and primitive types: [system.md](system.md) and [sml-deltas.md](sml-deltas.md)
- error conventions and `Result` layering: [error-handling.md](error-handling.md)
- modules and package-loading direction: [modules.md](modules.md)
- process lifecycle, join, wait-any, and cancellation: [runtime.md](runtime.md), [../crates/hiko-vm/src/process.rs](../crates/hiko-vm/src/process.rs), [../crates/hiko-vm/src/runtime.rs](../crates/hiko-vm/src/runtime.rs), [../crates/hiko-vm/src/threaded.rs](../crates/hiko-vm/src/threaded.rs)
- VM slice transitions, async suspension, and child creation: [vm.md](vm.md), [../crates/hiko-vm/src/vm/runtime_bridge.rs](../crates/hiko-vm/src/vm/runtime_bridge.rs), [../crates/hiko-vm/src/runtime_ops.rs](../crates/hiko-vm/src/runtime_ops.rs)
- current stdlib concurrency surface: [../libraries/Std-v0.1.0/modules/Fiber.hml](../libraries/Std-v0.1.0/modules/Fiber.hml)

### 1.3 Semantic layers

One recurring source of confusion in Hiko is mixing together language,
stdlib, VM, and runtime decisions. The ownership split is:

| Layer              | Owns                                                                        |
| ------------------ | --------------------------------------------------------------------------- |
| surface language   | syntax, types, patterns, modules, effect syntax                             |
| standard library   | user-facing conventions such as `Result` helpers and `Std.Fiber`            |
| VM                 | local execution state, bytecode dispatch, continuation capture, heap        |
| runtime            | process lifecycle, scheduling, async suspension, cancellation, capabilities |
| host configuration | policy inputs such as filesystem roots, HTTP allowlists, `exec` allowlists  |

If a question is really about ownership or state transitions, it usually
belongs to the VM/runtime layers, not the surface language.

## 2. Design Thesis

Hiko is aimed at scripting and tooling workloads where safety, predictability,
and direct style matter more than user-programmable concurrency semantics.
Typical workloads include:

- reading files and HTTP resources
- invoking external tools without a shell
- orchestrating document and code pipelines
- integrating host-native data capabilities such as cloud APIs or databases

The core thesis is simple:

1. Keep the language core small and explicit.
2. Keep effect handlers local to one process.
3. Keep suspension, scheduling, and authority in the runtime.
4. Keep heaps isolated per process.
5. Make process-boundary transfer explicit and typed.

This is intentionally not a programmable scheduler or a general shared-memory
concurrency platform. It is a fixed runtime model designed for safe
orchestration of external work.

### 2.1 Non-goals

Hiko is explicitly **not** trying to be:

- a shared-memory async runtime in the style of same-heap fibers
- an actor/message-passing language with mailboxes as the primary model
- a full SML-compatible language with every historical module feature
- a language with implicit numeric overloading or defaulting
- a platform where guest code defines scheduler or capability semantics

These exclusions are part of the design, not temporary omissions.

## 3. Core Language Design

### 3.1 Core semantics

Hiko is rooted in Core SML and keeps the parts that are still the best
foundation for a scripting language:

- strict call-by-value evaluation
- Hindley-Milner type inference
- value restriction
- algebraic data types
- exhaustive pattern matching
- immutable user bindings

Hiko should be understood as **SML-derived, not SML-obedient**. The point of
using Core SML is to inherit a strong semantic base, not to inherit every
historical ambiguity of SML'97.

### 3.2 Simplicity policy

Hiko deliberately prefers fewer semantic mechanisms:

- no overloaded numeric operators
- no type classes
- no exceptions as the primary recoverable-error mechanism
- no shared-memory concurrency surface
- no full recursive module tower
- no user-defined scheduler semantics

This is a readability choice as much as a compiler choice. Humans and agents
should be able to answer "what does this do?" without mentally expanding a
large amount of hidden dispatch.

### 3.3 Why the pipeline operator exists

Hiko includes a left-associative pipeline operator:

```sml
x |> f
```

which desugars to:

```sml
f x
```

The operator exists for three reasons:

- scripting code is often clearer left-to-right than deeply nested
- `Result`-style combinators become readable without method syntax
- it adds no new runtime semantics because it is pure desugaring

This is exactly the kind of convenience Hiko wants: small, obvious, and
semantically boring.

### 3.4 Numeric policy

Today the stable numeric story is intentionally small:

- current: `int` and `float` are the only builtin numeric surfaces documented
  as stable
- intended: keep numeric semantics explicit and avoid overloading/defaulting
- open: whether width-specific numeric modules such as `Float32` are added at
  all, and what exact API they use

- `int` for 64-bit signed integers
- `float` for 64-bit IEEE-754 floating point

Operators are monomorphic:

- `+`, `-`, `*`, `/`, `mod`, `<`, `<=`, `>`, `>=` for `int`
- `+.`, `-.`, `*.`, `/.`, `<.`, `<=.`, `>.`, `>=.` for `float`

Conversions are explicit:

- `int_to_float`
- `float_to_string`
- `floor`
- `ceil`

There is no ad-hoc overloading. That keeps inference simple and code review
obvious. A reader does not need to guess whether `+` dispatches through a type
class, an implicit conversion, or a trait-like mechanism.

Width-specific numerics such as `Float32` are **not** a stable part of the
language today. If Hiko adds them, the preferred direction is explicit library
or module APIs such as:

```sml
Float32.add x y
Float32.mul x y
Float32.from_float z
```

rather than reintroducing overloaded operators. The rule should stay the same:
new numeric domains should make their semantics more explicit, not less.

### 3.5 Equality and data reasoning

Equality is intentionally limited to equality-supporting types. This keeps
pattern matching and data reasoning clear and avoids pretending that every
runtime object has an obvious extensional equality story.

Bindings are immutable, so when Hiko code manipulates values it does so by
construction, pattern matching, and explicit return values, not by rebinding
mutable cells hidden behind familiar syntax.

### 3.6 Semantic cheat sheet

| Construct | Meaning |
| --- | --- |
| ``x \|> f`` | Pure desugaring to `f x` |
| `handle e with ...` | Install a local effect handler around `e` |
| `perform Tag v` | Invoke the nearest matching local handler for `Tag` |
| `resume k v` | Resume a captured continuation exactly once |
| `spawn (fn () => e)` | Create a new isolated child process to evaluate `e` |
| `Fiber.join child` | Wait for child completion and return `Result`, not a re-thrown child failure |
| `Fiber.cancel child` | Request cooperative child cancellation |
| `structure M = ...` | Introduce a compile-time namespace |
| `signature S = ...` | Describe a compile-time module interface |
| `use "./x.hml"` | Include local source by path |
| `import P.M` | Refer to a named package/module import surface |

## 4. Standard Data Types for Absence, Branching, and Failure

Hiko leans hard on ordinary algebraic data types.

### 4.1 `Option`

```sml
datatype 'a option =
    None
  | Some of 'a
```

`Option` means "value may be absent, and that absence is ordinary". It is the
right tool when failure details do not matter or when missing value is expected.

### 4.2 `Either`

```sml
datatype ('a, 'b) either =
    Left of 'a
  | Right of 'b
```

`Either` is for two ordinary alternatives. It is not specifically an error
type. A good use is to represent two successful parse forms or two transport
shapes where neither side is conceptually exceptional.

### 4.3 `Result`

```sml
datatype ('a, 'e) result =
    Ok of 'a
  | Err of 'e
```

`Result` is the standard Hiko answer to recoverable failure. This is a central
language-design decision:

- recoverable failure is data
- libraries define local `error` datatypes
- higher layers wrap lower-level errors with more context
- rendering is a boundary concern, not a library side effect

This keeps failure modes explicit in types and avoids turning everything into
strings or panic-style control flow.

### 4.4 Why Hiko does not lead with exceptions

Hiko has `panic` for unrecoverable failure, but it does not treat exceptions as
the main recoverable-error story. The language is trying to make orchestration
and review easy:

- typed errors are easier to inspect than ambient control transfer
- `Result` pipelines compose naturally with `|>`
- library APIs stay honest about what can fail

This matters even more once concurrency enters the picture. In Hiko, child
process failure is also surfaced as data rather than being secretly re-thrown
into the parent.

## 5. Module System and Code Composition

Hiko has a deliberately minimal module system.

Implemented today:

- top-level `signature`
- top-level `structure`
- transparent ascription `:`
- opaque ascription `:>`
- qualified names such as `List.fold` and `Queue.t`

Not implemented:

- `functor`
- `open`
- `where type`
- sharing constraints
- first-class modules
- nested modules as a general recursive tower

### 5.1 What modules are for

The module system exists to solve two practical problems:

- namespacing
- representation hiding

That is enough for a scripting language with growing libraries. Hiko does not
need a full higher-order module calculus just to expose a filesystem API, a
JSON API, or a few opaque data handles.

### 5.2 Compile-time only semantics

Modules are a front-end abstraction. They do not survive into the VM as runtime
module values:

- qualified names are resolved before code generation
- structures are flattened before bytecode emission
- runtime execution deals with ordinary functions, values, and bytecode

This matches Hiko's general rule: use the front end for source-level structure,
not the VM.

### 5.3 `use` vs `import`

The intended semantic split is:

- `use "./file.hml"` for explicit local file inclusion
- `import Std.List` for named package/module imports

This surface is still settling, but the semantic split matters more than the
exact keyword spelling:

- current: both `use` and `import` appear in repo docs and library sources
- current: the documented `import` shape is `Package.Module`, not an arbitrary
  deep module path
- current: the prelude is not assumed to be auto-imported
- intended: `use` stays path-local, while `import` is the package/module surface
- open: the exact packaging, fetch, cache, and lockfile mechanics

- local source composition should be path-oriented and explicit
- package/module loading should be named, resolved, and eventually verified

The published stdlib already leans toward `import`. The whitepaper treats that
direction as the intended long-term shape.

## 6. Effects: Local Control Flow, Not Global Scheduling

Hiko supports algebraic effects through `perform`, `handle`, and `resume`.

### 6.1 What effects mean

Effects are for **process-local structured control flow**:

- state threading
- generators
- early return / abort patterns
- local interpreters
- instrumentation or context propagation

They are not the language's async I/O substrate.

### 6.2 One-shot, local continuations

Continuations are one-shot and process-local.

When `perform` reaches a matching handler, the VM captures the stack and frames
between the perform site and that handler into a continuation object stored in
the current process heap. `resume` restores that continuation exactly once.

This is a deliberately constrained design:

- simpler than multi-shot continuations
- cheaper to reason about
- aligned with the fixed runtime model

The implementation uses deep continuation capture, so a continuation allocation
copies the relevant saved frames and stack segment. That is not free. Hiko
accepts that cost because effects are intended for structured local control, not
for replacing the scheduler.

### 6.3 Effects are not how Hiko does async

This is the key semantic split:

- effects are programmable local control flow
- async suspension is a fixed VM/runtime protocol

Some stdlib modules use local effects as a library structuring tool. For
example, `Std.Filesystem.run` handles local `ReadText`/`WriteText`-style
effects and delegates to raw builtins. But the actual suspension point, when
needed, still occurs at the builtin/runtime boundary. User code does not get to
redefine how I/O is scheduled.

## 7. Runtime Model

The system is organized around isolated processes.

Each process owns:

- its own VM instance
- its own heap
- its own operand stack and call frames
- its own handler stack and local continuations
- parent/scope metadata needed for structured concurrency

The runtime owns:

- the process table
- the scheduler
- the I/O backend
- capability policies and host integration

Compiled program data is immutable and may be shared across processes. Mutable
execution state is not.

### 7.1 Process state machine

At runtime, a process is not just "running" or "done". The current model is:

- `Runnable`: eligible to be scheduled
- `Blocked`: waiting on a child, any-of-child set, or an I/O token
- `Done`: finished successfully
- `Failed`: finished with `RuntimeError`, `HeapObjectLimitExceeded`,
  `FuelExhausted`, `Cancelled`, or a wrapped child failure

The important blocked reasons are:

- `Await { child, kind }`
- `WaitAny [child, ...]`
- `Io token`

This is the state machine that the runtimes actually manage. `Std.Fiber` is a
library layer over these runtime transitions; it is not a separate scheduler or
separate concurrency substrate.

The VM/runtime execution seam is likewise explicit. A scheduling slice ends in
one of these transition classes:

- `Done`
- `Yielded`
- `Failed`
- `Spawn`
- `Await` / `AwaitResult`
- `Cancel`
- `WaitAny`
- `Io`
- `Cancelled`

That seam matters because runtimes should react to these transitions, not reach
inside the VM and mutate interpreter state ad hoc.

### 7.2 Why VM values are `Copy`

The VM's `Value` type is a compact 16-byte `Copy` enum. Scalars live inline;
heap objects are referenced by `GcRef(u32)`. That gives Hiko the properties a
stack machine wants:

- cheap stack push/pop and local copies
- no reference counting on ordinary values
- no destructor interaction in the interpreter hot path
- simple GC rooting because values are plain data

The tradeoff is explicitness: anything heap-owned must live in the process heap,
and anything crossing a process boundary must be serialized into a different
representation.

### 7.3 Local heaps and process boundaries

Each process has its own heap and collector. There is no global heap and no
pointer from process A's heap into process B's heap.

Values crossing a process boundary are reified as `SendableValue`. That type
contains no `GcRef`, no closures, no continuations, and no RNG state. At the
boundary:

- the sending process serializes its value graph into `SendableValue`
- the receiving process deserializes it into its own heap

Strings and bytes use `Arc` leaves inside `SendableValue`, which reduces copying
while the value is in transit, but Hiko does **not** claim full end-to-end
zero-copy transfer. Deserializing into the child heap allocates fresh heap
objects.

### 7.4 Capability model and host authority

Hiko is not just isolated in memory. It is also intentionally host-governed in
authority.

The runtime owns capability policy for:

- filesystem roots and builtin folders
- allowed HTTP hosts
- allowed `exec` commands and timeouts
- stdio and other host integration points

Guest code does not mint new authority by importing a module, installing an
effect handler, or spawning a child. A child inherits the parent's capability
configuration when the runtime creates its fresh VM.

This matters semantically:

- `spawn` creates a new process, not a privilege boundary escape
- filesystem and HTTP access are constrained at the VM/runtime boundary
- `exec` is shell-free and whitelist-driven, not stringly shell evaluation
- capabilities are runtime configuration, not library-level convention

That is a major reason Hiko keeps async and capabilities in the runtime rather
than making them user-programmable effects.

### 7.5 Semantic invariants worth preserving

These are the highest-value invariants for implementation and review work. A
change that violates one of them is not a local tweak; it changes the design.

| Invariant                                                 | Why it matters                                                                        |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| No pointer from one process heap into another             | keeps GC, ownership, and transfer reasoning local                                     |
| Only `SendableValue` crosses process boundaries           | prevents accidental leakage of closures, continuations, or raw heap references        |
| Continuations are one-shot and process-local              | keeps effect semantics and resume behavior tractable                                  |
| Only the parent may await, cancel, or wait on a child     | preserves structured concurrency instead of drifting toward arbitrary process control |
| Child results are consumed once                           | allows prompt reclamation of finished child VM state                                  |
| Spawning a child does not grant new authority             | keeps capability enforcement host-owned                                               |
| The runtime observes VM state changes through `RunResult` | preserves the narrow VM/runtime seam                                                  |

If a proposed change weakens one of these, it should be treated as a language
design change and documented as such.

## 8. What `spawn` Means

`spawn` is not "start a new fiber in the same heap". It means "create a new
isolated child process".

Operationally, spawning a child does this:

1. The current process captures the target closure's code pointer and captures.
2. Captured values are serialized into `SendableValue`.
3. A fresh child VM is created, reusing immutable compiled program metadata and
   inheriting capability configuration from the parent.
4. Captures are deserialized into the child heap.
5. The child installs one initial call frame for the closure body.
6. The runtime inserts the child process into its process table and schedules it.

Important consequences:

- closures can be spawned, but their captures must be sendable
- closures, continuations, builtin functions, and RNG state do not cross the
  process boundary as captures
- the child starts with a fresh heap, fresh stack, and fresh handlers
- only immutable compiled program data and configuration are shared

If a capture cannot be serialized, spawning fails as a runtime error of the
spawning process. Hiko treats that as a semantic bug in the program's boundary
usage, not as an implicit deep-sharing escape hatch.

### 8.1 Single-threaded vs threaded runtime

The meaning of `spawn` is the same in both runtimes. The difference is only in
how child processes are executed:

- `Runtime` schedules them cooperatively on one OS thread
- `ThreadedRuntime` schedules them across worker threads and can overlap blocked
  I/O with other runnable processes

The language surface stays the same.

### 8.2 Process creation cost today

The source of truth for process creation cost is
[vm.md](vm.md#measuring-cost) and the benchmark example:

```bash
cargo run -p hiko-vm --example process_creation_cost --release
```

Current sample numbers from the development environment used for the refactor:

- `VM::create_child`: about `7.9 us/op`, `104 allocs/op`, `9836 B/op`
- full spawn path with zero captures: about `7.9 us/op`, `106 allocs/op`, `10012 B/op`
- full spawn path with four captures: about `8.1 us/op`, `110 allocs/op`, `10668 B/op`

Those numbers exclude scheduler/process-table insertion and focus on VM-side
creation.

What dominates the cost today:

- rebuilding builtin/global tables for the fresh VM
- cloning capability configuration
- deserializing captures into the child heap

What does **not** dominate:

- cloning compiled program structures, which are shared through `Arc`

For Hiko's target workload, this is acceptable. Child processes are not meant
to be nanosecond-level goroutines. They are isolated units of work whose
predictable teardown and ownership boundaries matter more than raw spawn count.

## 9. `Std.Fiber`: The User-Facing Structured Concurrency Layer

The raw runtime builtins operate on `pid`. The standard library layer is
`Std.Fiber`.

Today its core definition is effectively:

```sml
type 'a Fiber.t = pid
type error = BuiltinProcess.error
```

So `Fiber.t` is currently just a typed alias over `pid`, not an opaque runtime
handle. That may change in the future, but the semantics are already clear.

The layering is:

| Layer                | Current surface                                                                                    |
| -------------------- | -------------------------------------------------------------------------------------------------- |
| raw runtime builtins | `spawn`, `await_process`, `cancel`, `wait_any`, `await_result_raw`                                 |
| user-facing stdlib   | `Fiber.spawn`, `Fiber.join`, `Fiber.cancel`, `Fiber.first`, `Fiber.any`, `Fiber.both`, `Fiber.all` |

The source of truth for the user-facing semantics is
[../libraries/Std-v0.1.0/modules/Fiber.hml](../libraries/Std-v0.1.0/modules/Fiber.hml).
The source of truth for process lifecycle and join/cancel behavior is
[runtime.md](runtime.md) plus
[../crates/hiko-vm/src/process.rs](../crates/hiko-vm/src/process.rs),
[../crates/hiko-vm/src/runtime.rs](../crates/hiko-vm/src/runtime.rs), and
[../crates/hiko-vm/src/threaded.rs](../crates/hiko-vm/src/threaded.rs).

### 9.1 Core operations

`Fiber.spawn`

- type: `(unit -> 'a) -> 'a Fiber.t`
- meaning: create a child process and return its handle immediately

`Fiber.join`

- type: `'a Fiber.t -> ('a, Fiber.error) Result.result`
- meaning: wait for child completion and return success or process-level error

`Fiber.cancel`

- type: `'a Fiber.t -> unit`
- meaning: request cooperative cancellation; fire-and-forget

`Fiber.join_or_fail`

- meaning: convenience wrapper over `Fiber.join` that panics on `Err`

### 9.2 Higher-level combinators

`Fiber.both (f, g)`

- spawns both children
- joins both children
- returns `Result.Ok (left, right)` if both succeed
- returns the first observed `Result.Err` otherwise
- does **not** currently cancel the sibling on failure

`Fiber.all fs`

- spawns all children in the list
- joins them in list order
- returns `Result.Ok values` if all succeed
- returns the first observed `Result.Err` otherwise
- does **not** currently implement fail-fast sibling cancellation

`Fiber.first (f, g)`

- spawns both children
- waits for the first child to finish
- cancels the loser
- reaps the loser
- returns `Fiber.join` of the winner

`Fiber.any fs`

- spawns the whole list
- waits for the first finished child
- cancels all losers
- reaps all losers
- returns `Fiber.join` of the winner
- returns `Result.Err ...` on an empty list

These semantics are intentionally explicit. In particular, `first` and `any`
really are race-and-cancel combinators, while `both` and `all` are currently
"wait for everyone and report the first error" combinators.

### 9.3 `Fiber.join` and nested `Result`

`Fiber.join` does not throw the child's failure into the parent. It returns it.
That means layered failures remain visible in types.

Today the process-level error space exposed through `Fiber.join` is effectively:

- `RuntimeError message`
- `HeapObjectLimitExceeded (live, limit)`
- `FuelExhausted`
- `Cancelled`
- `AlreadyJoined`

That is narrower than "all possible things that went wrong in the host
process", and that is deliberate. `Fiber.error` describes join/process
outcomes, not every possible embedding failure.

For example, if a child computes:

```sml
(summary, App.error) Result.result
```

then joining it yields:

```sml
((summary, App.error) Result.result, Fiber.error) Result.result
```

That shape is sometimes verbose, but it is honest. The caller can then:

1. map `Fiber.error` into an application error
2. flatten the nested `Result`

This is easier to reason about than implicit cross-process exception tunneling.

## 10. Cancellation Semantics

Cancellation in Hiko is cooperative and boundary-based.

### 10.1 What cancellation is not

Cancellation is **not** asynchronous exception injection into arbitrary child
instruction streams. Hiko does not try to interrupt a child at an arbitrary
bytecode offset and unwind its frames from another thread.

### 10.2 What actually happens

If a child is blocked in the runtime:

- the runtime can mark it cancelled immediately
- the child becomes terminal with `Cancelled`
- waiters can be woken immediately

If a child is runnable or currently executing:

- the runtime records cancellation intent on the child VM
- the VM observes it at the next slice boundary
- the slice returns `RunResult::Cancelled`

In the threaded runtime, a child that is executing on another worker cannot be
mutated directly. The runtime records a pending cancel request, and the worker
applies it before the next slice it runs for that child.

### 10.3 Observable rules

The current observable semantics are:

- `Fiber.cancel` is fire-and-forget
- cancelling an already-finished child is a no-op
- a cancelled child eventually joins as `Result.Err Cancelled`
- only the parent may await/cancel/wait on its child
- child results are consumed once

Raw `await_process` and `await_result_raw` differ here:

- raw await can fail the parent when the child failed
- `Fiber.join` keeps process failure as a value

That split is intentional. The raw runtime API is low-level. The standard
library chooses the more compositional surface.

### 10.4 Scope cleanup

When a parent process terminates, the runtime cancels outstanding child
processes in its scope. This matters because Hiko wants process lifetimes to be
natural reclamation boundaries. The runtime should not keep orphaned child VMs
alive longer than their owner.

## 11. Hiko Async I/O vs Eio-Style Shared-Heap Async

Hiko borrows the direct-style goal from OCaml 5 / Eio, but it rejects the
shared-heap runtime model.

| Question                                    | Hiko                              | Eio-style shared-heap async                                |
| ------------------------------------------- | --------------------------------- | ---------------------------------------------------------- |
| Where does async suspension live?           | Fixed VM/runtime protocol         | Effect runtime in shared heap                              |
| What is the concurrency unit?               | Isolated process                  | Fiber in shared heap                                       |
| Can user code define scheduling semantics?  | No                                | More of the runtime semantics live in the effect substrate |
| How does data cross concurrency boundaries? | Explicit `SendableValue` transfer | Usually same-heap sharing                                  |
| Where are continuations valid?              | Inside one process only           | Inside one heap/fiber space                                |
| Who owns external authority?                | Host runtime and capabilities     | Depends on runtime/library design                          |

Hiko's answer is not "effects are bad". It is "effects should not also be the
place where capability ownership, cross-thread scheduling, and cross-process
memory reasoning are hidden".

This separation has practical consequences:

- I/O can stay direct-style without exposing scheduler internals
- the same guest code can run on a simple blocking runtime or a threaded async
  runtime
- capability checks remain centralized
- local reasoning about heaps stays local

## 12. Why This Arrangement Was Chosen

The design priorities are:

1. maintainability
2. simplicity
3. performance

That ordering is intentional.

### 12.1 Maintainability

The language should be easy to explain to humans and agents:

- errors are explicit in types
- module boundaries are simple
- concurrency goes through `Fiber`
- effects mean local control flow
- process boundaries are real ownership boundaries

### 12.2 Simplicity

The runtime has a narrow contract:

- VM owns execution-local state
- runtime owns scheduling, I/O, and capabilities
- `RunResult` is the transition seam

That separation is easier to document, test, and refactor than a shared-heap
system where continuations, scheduler state, and capability semantics are more
intertwined.

### 12.3 Performance

Performance still matters, but only after the meaning is obvious. Hiko makes a
few specific performance bets:

- `Copy` VM values keep stack manipulation cheap
- per-process GC avoids global heap contention
- immutable compiled program structures are shared across children
- short-lived whole-process reclamation is often more important than heroic
  long-lived-heap tuning

The design accepts some costs in return:

- spawning has microsecond-scale setup cost
- process-boundary transfer allocates
- deep continuations allocate when effects capture stack state

These are acceptable tradeoffs for the target workload.

### 12.4 Review checklist for language and runtime changes

When reviewing a change against this design, ask:

1. Does it keep recoverable failure explicit in types, or does it smuggle error
   handling into ambient control flow?
2. Does it preserve the split between local effects and runtime-managed async?
3. Does it preserve process isolation, `SendableValue` boundaries, and
   capability non-escalation?
4. Does it make the surface language more explicit, or does it add hidden
   dispatch such as overloading, implicit sharing, or implicit authority?
5. If it changes `Fiber`, `spawn`, join, or cancellation behavior, is that
   reflected in both stdlib docs and runtime invariants?

If the answer to one of these is "no" or "not sure", the change probably needs
design-level discussion rather than a local implementation tweak.

## 13. Open Ends and Future Work

Several areas are intentionally unfinished:

- exact package-loading syntax and packaging workflow
- typed fiber/process handles instead of raw `pid`
- effect typing
- further stdlib design polish
- width-specific numeric modules such as `Float32`, if and when they are added

The important constraint is that future work should preserve the core semantic
shape:

- local effects remain local
- runtime authority remains host-owned
- cross-process sharing remains explicit
- new conveniences should reduce boilerplate without adding hidden semantics

### 13.1 Current vs intended vs open

The highest-risk documentation failure for Hiko is to blur implemented behavior
with intended direction. The current boundary is:

| Topic                                      | Implemented today                                                              | Intended direction                                        | Still open                                                                             |
| ------------------------------------------ | ------------------------------------------------------------------------------ | --------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Local source composition                   | `use "./file.hml"` style inclusion exists in repo examples and docs            | keep local composition path-based and explicit            | exact long-term package/source split UX                                                |
| Named package imports                      | `import Package.Module` is documented and used in the published stdlib sources | converge on named package imports for stdlib and packages | loader, packaging, lockfile, and fetch workflow details                                |
| `Std.Fiber` handle shape                   | `'a Fiber.t = pid` in the current stdlib                                       | likely move toward a more explicit typed handle story     | whether that becomes opaque in the surface language                                    |
| Recoverable errors                         | `Std.Result` plus library-owned `error` datatypes                              | keep this as the default application/library style        | whether any effect-typed error surface is ever added                                   |
| Numeric operators                          | monomorphic `int` and `float` operators                                        | keep no-overloading as the default rule                   | whether width-specific numeric modules such as `Float32` land, and with what exact API |
| Effects                                    | one-shot local handlers with deep continuation capture                         | keep effects local, not the async substrate               | whether effect typing is added                                                         |
| Async I/O                                  | runtime-managed suspension via builtins and `RunResult`/`RuntimeRequest`       | preserve fixed runtime ownership of async behavior        | backend and packaging evolution, not the semantic split                                |
| Process handles and structured concurrency | raw runtime builtins plus `Std.Fiber` library layer                            | keep `Fiber` as the main user-facing concurrency surface  | exact future surface for typed handles and richer combinators                          |

## 14. Conclusion

Hiko explores a design in which algebraic effects and isolated-process
execution are deliberately assigned different responsibilities. Effects provide
programmable local control flow. The runtime provides fixed suspension,
capability enforcement, scheduling, cancellation boundaries, and process-boundary
transfer.

The result is a language/runtime architecture aimed at safe orchestration:

- direct style inside a process
- explicit ownership across processes
- typed recoverable failure
- fixed, auditable runtime semantics
- a language surface that stays intentionally small enough to explain

That is the point of Hiko's design. It is not trying to be the most general
concurrency substrate. It is trying to be the clearest one that still covers
real scripting and agent workloads.
