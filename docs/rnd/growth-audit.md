# Growth audit

Accidental quadratic or worse code, measured on `main` at `e299c91`. Times are
from a release build on macOS, and each row was timed at several sizes.
Automated reviewers ran these; the lead reproduced only the shadowing bug below.
`Fiber.all` is in `docs/rnd/fiber-join-growth.md` (#195). The VM audit is not
finished, so this page does not cover `crates/hiko-vm`.

## A wrong answer

`declare_global` (`compiler.rs:1572`) can reuse a `x$n` name, so a later shadow
overwrites a value that earlier code reads. See #199.

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

`string_length` is O(n) and `substring` is O(start), so any loop that indexes a
string by position is quadratic.

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
  compiler rows are fixed.
- `growth.rs` counts only type work, so it missed the constructor clone.
- `Issue.action_json` does not appear to escape strings in its JSON. Read, not
  run.
