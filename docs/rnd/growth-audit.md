# Growth audit

Accidental quadratic or worse code, measured on `main` at `e299c91`. Times are
from a release build on macOS, and each row was timed at several sizes.
Automated reviewers ran these; the lead reproduced only the shadowing bug below.
`Fiber.all` is in `docs/rnd/fiber-join-growth.md` (#195).

## Two correctness bugs

- `declare_global` (`compiler.rs:1572`) can reuse a `x$n` name, so a later
  shadow overwrites a value that earlier code reads. See #199.
- The heap loses free slots across sweeps (`heap.rs:1130`), so a program within
  `max_heap` can be refused. See #201.

## Runtime

Timed with a raised fuel and heap limit, on one worker.

| Where | Grows with | Timings | Fix |
|---|---|---|---|
| `builtins/json.rs:96,135,176` | document size × calls; each `json_get` reparses | 1,000, 8,000 lookups: 152 ms, 17.9 s | cache the parsed value, like `RegexCache` |
| `builtins/string.rs:86,56` | string length × calls; `substring` and `string_length` scan UTF-8 | `substring` loop 20,000, 160,000: 198 ms, 11.2 s | one-entry cursor in `Heap` |
| `vm/gc.rs:17`, `heap.rs:817` | live objects / room left; a full GC each `room` allocations | `max_heap` 1M, room 100,000, 1,000: 879 ms, 26 s; room 100 passes 120 s | fail when a GC frees under 1% of the limit |
| `threaded.rs:457` | parked fibers × I/O events; the monitor scans the process table | 1,000, 8,000: 33 ms, 949 ms | test `io_waiters.is_empty()` first; tested |
| `vm/child_globals.rs:57-61` | top-level `val x = spawn ...` lines | 2,000, 8,000: 172 ms, 2.2 s | delta on the last snapshot |
| `vm/dispatch.rs:640,676,571` | inner handlers × `perform`s; capped at 65,536 | 1,000, 8,000: 35 ms, 600 ms | track a visible-top index |
| `scheduler.rs:84` | run queue × terminations | 40,000, 80,000: 293 ms, 1.1 s; 100 ms at the default cap | delete the call: by reading, it never matches |

The GC path charges no fuel or host work (read, not run), so a sandboxed script
can burn CPU near the heap limit.

Suspected, read or not clearly quadratic: `remove_queued` in `io_backend.rs:355`,
the memory-bytes branch at `heap.rs:817`, register VM `materialize`.

Linear: `split`, `string_join`, `string_replace`, `bytes_get`, `bytes_slice`,
cached `regex_match`, spawn then join, sending a list. Not a bug: `s ^ x` in a
loop is quadratic because it copies; `string_join` is the linear route.

## Compiler

| Where | Grows with | Timings | Fix |
|---|---|---|---|
| `exhaustive.rs:248` | exponential: columns of a tuple `case`; no budget, so it can hang | bool columns 14, 18, 20: 165 ms, 3.9 s, 36 s | charge a budget; stop at an all-wildcard row |
| `infer.rs:1798` | type aliases are not charged to `max_type_work` | `type ti = t(i-1) * t(i-1)`, n 20, 24: 238 ms, 3.5 s, accepted | let `substitute_vars` charge the work |
| `infer.rs:1942`, `exhaustive.rs:81` | cases × constructors | 4,000, 16,000: 980 ms, 16.7 s | borrow the list, or `Arc<[..]>` |
| `compiler.rs:2308` | imports × symbols | 1,000, 4,000 `use`: 226 ms, 4.2 s | one interner |
| `compiler.rs:1572` | shadowed names × globals | 4,000, 16,000: 58 ms, 1.4 s | per-name counter (#199) |
| `exhaustive.rs:351` | list patterns: about ×14 per doubling | 100, 200: 565 ms, 9.1 s; rejected later | `Rc` subpatterns |
| `exhaustive.rs:209` | arms × earlier arms | `(Ci,Ci)` 1,000, 4,000: 169 ms, 2.7 s | bucket by head constructor |
| `format.rs:210,225`, `pretty.rs:142,154` | comments × declarations | 4,000, 16,000: 57 ms, 684 ms | `partition_point` |
| `compiler.rs:1411` | locals × reads in a `let` | 8,000, 32,000: 46 ms, 523 ms | name to slot stack |
| `exhaustive.rs:298` | datatypes × nested patterns | 2,000, 8,000: 73 ms, 604 ms | constructor to datatype map |
| `infer.rs:614` | alias depth | 500, 1,000: 71 ms, 315 ms; 2,000 hits the limit | resolve only the head |
| `desugar.rs:572,596` | binders in a structure | 64,000, 128,000: 620 ms, 2.1 s | scoped environment |

Suspected, read but not run: `verify.rs:710` on crafted bytecode.

Linear: top-level `val`s, functions, datatypes, list and string literals, the
constant pool, verifier on compiled programs, generalization, nesting to the
256 limit, `fmt` without comments.

## Libraries

| Where | Grows with | Timings | Fix |
|---|---|---|---|
| `Sre Report.hml:27-38` | checks | `Report.ordered` 500, 1,500: 76 ms, 586 ms; fuel runs out at 2,000 | filter per status and append; tested: 38 ms at 8,000 |
| `Std Format.hml:39-74` | width | `pad_right` 40,000, 160,000: 156 ms, 2.8 s | `repeat` by doubling |
| `Std Format.hml:18-33` | template length | 32,000, 128,000: 595 ms, 8.4 s | split on `%`, then `string_join` |
| `Std Result.hml:168-181` | errors, in a left fold over `apply` | 500, 2,000: 57 ms, 697 ms; fuel runs out at 3,000 | point to `validate_all` |
| `Std Format.hml:37` | n | 6,000, 24,000: 71 ms, 986 ms | as `pad_right` |
| `Issue.hml:59-63` | actions | 4,000, 8,000: 52 ms, 175 ms | map, then `string_join` |

`Report` is the likely first hit: an all-OK fleet of 2,000 checks. Nothing on
`main` calls `Format.format`, `pad_right` or `column`.

## Stack limits

The value stack has 64K slots (`vm/mod.rs:48`), so non-tail recursion overflows.
Fails at about: `List.zip`, `Log.kv` 5,100; `Report.insert` 6,700; `Result.all`
and `traverse` 7,400; `List.foldr` 8,300; `List.map`, `filter`, `append`, `take`,
`Result.validate_all` and `FiberInternal.spawn_all` 9,500; `List.length` 10,900.

A 10,000-row list is ordinary, so this is more likely to be hit than most of the
quadratic paths above. The fix is an accumulator, which costs a second
allocation for `map`-like functions. `List.length` needs only the accumulator.

## Concerns

- Compile time is a denial of service for untrusted source until the first two
  compiler rows are fixed. The GC row is the same risk at run time.
- `growth.rs` counts only type work, so it missed the constructor clone.
- `Issue.action_json` does not appear to escape strings in its JSON. Read, not
  run.
